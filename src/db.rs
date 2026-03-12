use std::fs::{File, OpenOptions};
use std::io::{Error, Seek, SeekFrom};
use std::sync::Mutex;

use crate::hash_index::HashIndex;
use crate::record::{
    MAX_KEY_SIZE, MAX_VALUE_SIZE, Record, RecordHeader, append_record, read_record,
};
use crate::segment::{Segment, get_last_segment};

pub struct DB {
    index: HashIndex,
    db_file: Mutex<File>,
    db_path: String,
    db_name: String,
}

impl DB {
    pub fn new(db_path: &str, db_name: &str) -> DB {
        std::fs::create_dir_all(db_path).unwrap();
        let segment = Segment::new(db_name).unwrap();
        let file: File = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(segment.path(db_path))
            .unwrap();

        DB {
            index: HashIndex::new(),
            db_file: Mutex::new(file),
            db_path: db_path.to_string(),
            db_name: db_name.to_string(),
        }
    }

    pub fn from_dir(db_dir: &str, db_name: &str) -> Result<Option<DB>, Error> {
        let segment = match get_last_segment(db_dir, db_name) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        match segment {
            Some(segment) => {
                let mut file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(segment.path(db_dir))?;
                Ok(Some(DB {
                    index: HashIndex::from_file(&mut file)?,
                    db_file: Mutex::new(file),
                    db_path: db_dir.to_string(),
                    db_name: db_name.to_string(),
                }))
            }
            None => Ok(None),
        }
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
        let offset = match self.index.get(key) {
            Some(o) => *o,
            None => return Ok(None),
        };
        let mut file = self.db_file.lock().unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();

        let record = read_record(&mut *file)?;

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
        let mut file = self.db_file.lock().unwrap();
        let record = Record {
            header: RecordHeader {
                key_size: key.len() as u64,
                value_size: value.len() as u64,
                tombstone: false,
            },
            key: key.to_string(),
            value: value.to_string(),
        };
        let offset = append_record(&mut *file, &record)?;
        self.index.set(key.to_string(), offset);

        Ok(())
    }

    pub fn delete(&mut self, key: &str) -> Result<Option<()>, Error> {
        if key.len() > MAX_KEY_SIZE {
            return Err(Error::new(
                std::io::ErrorKind::InvalidInput,
                "key or value exceeds maximum allowed size",
            ));
        }
        let mut file = self.db_file.lock().unwrap();
        match self.index.delete(key) {
            Some(_) => {
                let record = Record {
                    header: RecordHeader {
                        key_size: key.len() as u64,
                        value_size: 0,
                        tombstone: true,
                    },
                    key: key.to_string(),
                    value: String::new(),
                };
                append_record(&mut *file, &record)?;
                Ok(Some(()))
            }
            None => Ok(None),
        }
    }

    pub fn get_compacted(&self) -> Result<DB, Error> {
        let mut new_db = DB::new(&self.db_path, &self.db_name);

        let keys: Vec<String> = self.index.ls_keys().cloned().collect();
        for k in keys {
            let value = match self.get(&k)? {
                Some((_, value)) => value,
                None => continue,
            };
            new_db.set(&k, &value)?;
        }

        Ok(new_db)
    }
}
