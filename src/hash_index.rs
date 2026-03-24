use std::{
    collections::HashMap,
    fs::File,
    io::{Error, Seek, SeekFrom},
};

use crate::record::Record;

/// In-memory index mapping keys to their byte offsets in the database file.
pub struct HashIndex {
    hashmap: HashMap<String, IndexEntry>,
}

pub struct IndexEntry {
    pub segment_timestamp: u64,
    pub offset: u64,
    pub record_size: u64,
}

impl HashIndex {
    /// Creates an empty index.
    pub(crate) fn new() -> HashIndex {
        HashIndex {
            hashmap: HashMap::new(),
        }
    }

    /// Returns the byte offset for the given key, or `None` if it is not present.
    pub fn get(&self, key: &str) -> Option<&IndexEntry> {
        self.hashmap.get(key)
    }

    /// Inserts or updates the byte offset for the given key.
    pub fn set(
        &mut self,
        key: String,
        file_location: u64,
        segment_timestamp: u64,
        record_size: u64,
    ) -> Option<IndexEntry> {
        let entry = IndexEntry {
            segment_timestamp,
            offset: file_location,
            record_size,
        };
        self.hashmap.insert(key, entry)
    }

    /// Returns an iterator over all keys in the index.
    pub fn ls_keys(&self) -> impl Iterator<Item = &String> {
        self.hashmap.keys()
    }

    /// Removes a key from the index, returning its byte offset if it was present.
    pub fn delete(&mut self, key: &str) -> Option<IndexEntry> {
        self.hashmap.remove(key)
    }

    /// Rebuilds the index by sequentially scanning all records in the given
    /// database file. Tombstoned records are skipped. For duplicate keys,
    /// the last occurrence wins.
    pub fn from_file(file: &mut File, segment_timestamp: u64) -> Result<HashIndex, Error> {
        let mut hashmap = HashMap::new();
        let file_size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;

        while file.stream_position()? < file_size {
            let offset = file.stream_position()?;
            let record = Record::read_next(file)?;
            let header = record.header;

            if header.tombstone {
                hashmap.remove(&record.key);
                continue;
            }
            let key = record.key;
            let entry = IndexEntry {
                segment_timestamp,
                offset,
                record_size: 0,
            };
            hashmap.insert(key, entry);
        }

        Ok(HashIndex { hashmap })
    }

    pub fn merge_from_file(
        &mut self,
        file: &mut File,
        segment_timestamp: u64,
    ) -> Result<(), Error> {
        let file_size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;

        while file.stream_position()? < file_size {
            let offset = file.stream_position()?;
            let record = Record::read_next(file)?;
            let header = record.header;

            if header.tombstone {
                self.hashmap.remove(&record.key);
                continue;
            }
            let key = record.key;
            let entry = IndexEntry {
                segment_timestamp,
                offset,
                record_size: 0,
            };
            self.hashmap.insert(key, entry);
        }
        Ok(())
    }
}
