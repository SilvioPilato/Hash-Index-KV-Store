use std::{
    fs::{File, OpenOptions, read_dir},
    io::{self, BufReader, ErrorKind, Seek},
    path::PathBuf,
    time::SystemTime,
};

use crate::{
    bloom::BloomFilter,
    memtable::Memtable,
    record::{Record, RecordHeader},
};

const SPARSE_INDEX_INTERVAL: usize = 64;
const BLOOM_BITS_PER_KEY: usize = 10;
const BLOOM_HASH_COUNT: u32 = 7;

pub struct SSTable {
    pub path: PathBuf,
    pub timestamp: u64, // for ordering segments newest-to-oldest
    name: String,
    sparse_index: Vec<(String, u64)>,
    bloom: BloomFilter,
}

pub struct SSTableIter {
    file: BufReader<File>,
}

impl Iterator for SSTableIter {
    type Item = io::Result<Record>;

    fn next(&mut self) -> Option<Self::Item> {
        match Record::read_next(&mut self.file) {
            Ok(record) => Some(Ok(record)),
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => None,
            Err(e) => Some(Err(e)),
        }
    }
}

impl SSTable {
    /// Flush a memtable to disk as a sorted segment file
    pub fn from_memtable(dir: &str, name: &str, memtable: &Memtable) -> io::Result<SSTable> {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| io::Error::other("SystemTime error"))?
            .as_nanos() as u64;
        let filename = format!("{}_{}.sst", name, timestamp);
        let path = PathBuf::from(dir).join(filename);

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut sparse_index: Vec<(String, u64)> = Vec::new();
        let key_count = memtable.entries().len();
        let bloom_bytes = (key_count * BLOOM_BITS_PER_KEY).div_ceil(8);
        let mut bloom = BloomFilter::new(bloom_bytes.max(1), BLOOM_HASH_COUNT);
        for (i, (key, opt)) in memtable.entries().iter().enumerate() {
            let (value, tombstone) = match opt {
                Some(v) => (v.to_string(), false),
                None => (String::new(), true),
            };
            let record = Record {
                header: RecordHeader {
                    crc32: 0u32,
                    key_size: key.len() as u64,
                    value_size: value.len() as u64,
                    tombstone,
                },
                key: key.to_string(),
                value,
            };

            let offset = record.append(&mut file)?;
            if i % SPARSE_INDEX_INTERVAL == 0 {
                sparse_index.push((key.to_owned(), offset));
            }
            bloom.insert(key);
        }
        Ok(SSTable {
            path,
            timestamp,
            name: name.to_string(),
            sparse_index,
            bloom,
        })
    }

    /// Scan the file for a key, return the value (or None/tombstone)
    pub fn get(&self, key: &str) -> io::Result<Option<Option<String>>> {
        if !self.bloom.might_contain(key) {
            return Ok(None);
        }
        let offset = self.get_offset(key);

        let file = OpenOptions::new().read(true).open(&self.path)?;
        let mut reader = BufReader::new(file);
        reader.seek(io::SeekFrom::Start(offset))?;

        let iter = SSTableIter { file: reader };

        for result in iter {
            let record = result?;
            match record.key.as_str().cmp(key) {
                std::cmp::Ordering::Greater => return Ok(None),
                std::cmp::Ordering::Equal => {
                    let value = if record.header.tombstone {
                        None
                    } else {
                        Some(record.value)
                    };
                    return Ok(Some(value));
                }
                std::cmp::Ordering::Less => continue,
            }
        }
        Ok(None)
    }

    /// Iterate all records in order (for compaction merging)
    pub fn iter(&self) -> io::Result<SSTableIter> {
        let file = OpenOptions::new().read(true).open(&self.path)?;
        Ok(SSTableIter {
            file: BufReader::new(file),
        })
    }

    pub fn parse(filename: &str) -> Option<Self> {
        let stem = filename.strip_suffix(".sst")?;
        let (name, ts) = stem.rsplit_once('_')?;
        let timestamp = ts.parse().ok()?;
        let path = PathBuf::new();
        let sparse_index: Vec<(String, u64)> = Vec::new();
        let bloom = BloomFilter::new(1, BLOOM_HASH_COUNT);
        Some(Self {
            path,
            timestamp,
            name: name.to_string(),
            sparse_index,
            bloom,
        })
    }

    /// Rebuild the sparse index and Bloom filter by scanning all records.
    fn rebuild_index(&mut self) -> io::Result<()> {
        let file = OpenOptions::new().read(true).open(&self.path)?;
        let mut reader = BufReader::new(file);
        let mut sparse_index: Vec<(String, u64)> = Vec::new();
        let mut keys: Vec<String> = Vec::new();
        let mut i = 0usize;
        loop {
            let offset = reader.stream_position()?;
            match Record::read_next(&mut reader) {
                Ok(record) => {
                    if i.is_multiple_of(SPARSE_INDEX_INTERVAL) {
                        sparse_index.push((record.key.clone(), offset));
                    }
                    keys.push(record.key);
                    i += 1;
                }
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            }
        }
        let bloom_bytes = (keys.len() * BLOOM_BITS_PER_KEY).div_ceil(8);
        let mut bloom = BloomFilter::new(bloom_bytes.max(1), BLOOM_HASH_COUNT);
        for key in &keys {
            bloom.insert(key);
        }
        self.sparse_index = sparse_index;
        self.bloom = bloom;
        Ok(())
    }

    fn get_offset(&self, key: &str) -> u64 {
        let pos = self
            .sparse_index
            .partition_point(|(k, _)| k.as_str() <= key);
        if pos > 0 {
            self.sparse_index[pos - 1].1 // seek to this offset
        } else {
            0 // start of file
        }
    }
}

pub fn get_sstables(dir: &str, db_name: &str) -> io::Result<Vec<SSTable>> {
    let mut tables: Vec<SSTable> = read_dir(dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            let mut table = SSTable::parse(&name)?;
            table.path = PathBuf::from(dir).join(&name);
            if table.name == db_name {
                Some(table)
            } else {
                None
            }
        })
        .collect();
    tables.sort_by_key(|s| s.timestamp);
    for table in &mut tables {
        table.rebuild_index()?;
    }
    Ok(tables)
}
