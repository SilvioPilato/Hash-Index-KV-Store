use std::fs::{self, File, OpenOptions};
use std::io::{self, Error, Seek, SeekFrom};

use crate::hash_index::HashIndex;
use crate::record::{
    MAX_KEY_SIZE, MAX_VALUE_SIZE, Record, RecordHeader, append_record, read_record,
};
use crate::segment::{Segment, get_segments};
use crate::settings::FSyncStrategy;

pub struct DB {
    index: HashIndex,
    active_file: File,
    db_path: String,
    db_name: String,
    active_segment: Segment,
    max_segment_bytes: u64,
    writes_since_fsync: u64,
    fsync_strategy: FSyncStrategy,
}

impl DB {
    pub fn new(
        db_path: &str,
        db_name: &str,
        max_segment_bytes: u64,
        fsync_strategy: FSyncStrategy,
    ) -> DB {
        std::fs::create_dir_all(db_path).unwrap();
        let segment = Segment::new(db_name).unwrap();
        let file: File = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(segment.path(db_path))
            .unwrap();

        DB {
            index: HashIndex::new(),
            active_file: file,
            db_path: db_path.to_string(),
            db_name: db_name.to_string(),
            active_segment: segment,
            max_segment_bytes,
            writes_since_fsync: 0,
            fsync_strategy,
        }
    }

    pub fn from_dir(
        db_dir: &str,
        db_name: &str,
        max_segment_bytes: u64,
        fsync_strategy: FSyncStrategy,
    ) -> Result<Option<DB>, Error> {
        let segments = match get_segments(db_dir, db_name) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };

        if segments.is_empty() {
            return Ok(None);
        }

        let mut hash_index = HashIndex::new();
        let mut current_file = None;
        let mut active_segment = None;

        for segment in segments.iter() {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(segment.path(db_dir))?;
            hash_index.merge_from_file(&mut file, segment.timestamp)?;
            current_file = Some(file);
            active_segment = Some(segment);
        }
        Ok(Some(DB {
            index: hash_index,
            active_file: current_file.unwrap(),
            db_path: db_dir.to_string(),
            db_name: db_name.to_string(),
            active_segment: Segment {
                segment_name: active_segment.unwrap().segment_name.clone(),
                timestamp: active_segment.unwrap().timestamp,
            },
            max_segment_bytes,
            writes_since_fsync: 0,
            fsync_strategy,
        }))
    }

    /// Retrieves the value associated with the given key.
    ///
    /// Looks up the key in the in-memory index to find the byte offset in the
    /// database file, then reads the entry using the format:
    /// `|size_k (8 bytes BE u64)|size_v (8 bytes BE u64)|key (raw UTF-8)|value (raw UTF-8)|`
    /// Skips past the key bytes and returns the value.
    ///
    /// Returns `None` if the key is not in the index or if the stored bytes
    /// are not valid UTF-8.
    pub fn get(&self, key: &str) -> Result<Option<(String, String)>, Error> {
        let entry = match self.index.get(key) {
            Some(o) => o,
            None => return Ok(None),
        };

        let mut file = if entry.segment_timestamp == self.active_segment.timestamp {
            self.active_file.try_clone()?
        } else {
            let segment = Segment {
                segment_name: self.db_name.clone(),
                timestamp: entry.segment_timestamp,
            };
            File::open(segment.path(&self.db_path))?
        };

        file.seek(SeekFrom::Start(entry.offset)).unwrap();

        let record = read_record(&mut file)?;

        Ok(Some((record.key, record.value)))
    }

    /// Inserts or updates a key-value pair in the database.
    ///
    /// Appends an entry to the end of the database file using the format:
    /// `|size_k (8 bytes BE u64)|size_v (8 bytes BE u64)|key (raw UTF-8)|value (raw UTF-8)|`
    /// with no separators between fields. Then records the byte offset of
    /// this new entry in the in-memory index under the given key.
    ///
    /// If the key already existed, the old entry remains as dead bytes in the
    /// file (reclaimed later by compaction) and the index is updated to point
    /// to the new one.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), Error> {
        if key.len() > MAX_KEY_SIZE || value.len() > MAX_VALUE_SIZE {
            return Err(Error::new(
                std::io::ErrorKind::InvalidInput,
                "key or value exceeds maximum allowed size",
            ));
        }
        let record = Record {
            header: RecordHeader {
                crc32: 0u32,
                key_size: key.len() as u64,
                value_size: value.len() as u64,
                tombstone: false,
            },
            key: key.to_string(),
            value: value.to_string(),
        };

        let seg_size = self.active_file.seek(SeekFrom::End(0))?;

        if self.max_segment_bytes < seg_size + record.size_on_disk() {
            self.roll_segment()?;
        }
        let mut file = self.active_file.try_clone()?;
        let offset = append_record(&mut file, &record)?;
        self.index
            .set(key.to_string(), offset, self.active_segment.timestamp);
        self.fsync()?;

        Ok(())
    }

    pub fn delete(&mut self, key: &str) -> Result<Option<()>, Error> {
        if key.len() > MAX_KEY_SIZE {
            return Err(Error::new(
                std::io::ErrorKind::InvalidInput,
                "key or value exceeds maximum allowed size",
            ));
        }
        let mut file = self.active_file.try_clone()?;
        match self.index.delete(key) {
            Some(_) => {
                let record = Record {
                    header: RecordHeader {
                        crc32: 0u32,
                        key_size: key.len() as u64,
                        value_size: 0,
                        tombstone: true,
                    },
                    key: key.to_string(),
                    value: String::new(),
                };
                append_record(&mut file, &record)?;
                self.fsync()?;
                Ok(Some(()))
            }
            None => Ok(None),
        }
    }

    pub fn get_compacted(&self) -> Result<DB, Error> {
        let old_segments = get_segments(&self.db_path, &self.db_name)?;
        let mut new_db = DB::new(
            &self.db_path,
            &self.db_name,
            self.max_segment_bytes,
            self.fsync_strategy,
        );

        let keys: Vec<String> = self.index.ls_keys().cloned().collect();
        for k in keys {
            let value = match self.get(&k)? {
                Some((_, value)) => value,
                None => continue,
            };
            new_db.set(&k, &value)?;
        }
        new_db.active_file.sync_all()?;

        for segment in &old_segments {
            fs::remove_file(segment.path(&self.db_path))?;
        }

        Ok(new_db)
    }

    fn roll_segment(&mut self) -> io::Result<()> {
        let segment = Segment::new(&self.db_name).unwrap();
        let file: File = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(segment.path(&self.db_path))?;
        self.active_file = file;
        self.active_segment = segment;
        Ok(())
    }

    fn fsync(&mut self) -> io::Result<()> {
        self.writes_since_fsync += 1;
        match self.fsync_strategy {
            FSyncStrategy::Always => {
                self.writes_since_fsync = 0;
                self.active_file.sync_all()?
            }
            FSyncStrategy::Never => {}
            FSyncStrategy::EveryN(n) => {
                if n <= self.writes_since_fsync as usize {
                    self.writes_since_fsync = 0;
                    self.active_file.sync_all()?
                }
            }
        }

        Ok(())
    }
}
