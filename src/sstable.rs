use std::{
    fs::{File, OpenOptions, read_dir},
    io::{self, BufReader, Seek},
    path::PathBuf,
    time::SystemTime,
};

use crate::{
    memtable::Memtable,
    record::{Record, RecordHeader},
};

const SPARSE_INDEX_INTERVAL: usize = 64;

pub struct SSTable {
    pub path: PathBuf,
    pub timestamp: u64, // for ordering segments newest-to-oldest
    name: String,
    sparse_index: Vec<(String, u64)>,
}

pub struct SSTableIter {
    file: BufReader<File>,
}

impl Iterator for SSTableIter {
    type Item = Record;

    fn next(&mut self) -> Option<Self::Item> {
        Record::read_next(&mut self.file).ok()
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
        }
        Ok(SSTable {
            path,
            timestamp,
            name: name.to_string(),
            sparse_index,
        })
    }

    /// Scan the file for a key, return the value (or None/tombstone)
    pub fn get(&self, key: &str) -> io::Result<Option<Option<String>>> {
        let offset = self.get_offset(key);

        let file = OpenOptions::new().read(true).open(&self.path)?;
        let mut reader = BufReader::new(file);
        reader.seek(io::SeekFrom::Start(offset))?;

        let iter = SSTableIter { file: reader };

        for record in iter {
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
        Some(Self {
            path,
            timestamp,
            name: name.to_string(),
            sparse_index,
        })
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
    Ok(tables)
}
