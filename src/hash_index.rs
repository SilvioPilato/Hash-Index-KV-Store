use std::{
    collections::HashMap,
    fs::File,
    io::{Error, ErrorKind, Read, Seek, SeekFrom},
};

use crate::utils::read_record_header;

/// In-memory index mapping keys to their byte offsets in the database file.
pub struct HashIndex {
    hashmap: HashMap<String, u64>,
}

impl HashIndex {
    /// Creates an empty index.
    pub(crate) fn new() -> HashIndex {
        let hashmap = HashMap::new();
        return HashIndex { hashmap };
    }

    /// Returns the byte offset for the given key, or `None` if it is not present.
    pub fn get(&self, key: &str) -> Option<&u64> {
        self.hashmap.get(key)
    }

    /// Inserts or updates the byte offset for the given key.
    pub fn set(&mut self, key: String, location: u64) {
        self.hashmap.insert(key, location);
    }

    /// Returns an iterator over all keys in the index.
    pub fn ls_keys(&self) -> std::collections::hash_map::Keys<'_, String, u64> {
        self.hashmap.keys()
    }

    /// Removes a key from the index, returning its byte offset if it was present.
    pub fn delete(&mut self, key: &str) -> Option<u64> {
        self.hashmap.remove(key)
    }

    /// Rebuilds the index by sequentially scanning all records in the given
    /// database file. Tombstoned records are skipped. For duplicate keys,
    /// the last occurrence wins.
    pub fn from_file(file: &mut File) -> Result<HashIndex, Error> {
        let mut hashmap = HashMap::new();
        let file_size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;

        while file.seek(SeekFrom::Current(0))? < file_size {
            let record_offset = file.seek(SeekFrom::Current(0))?;
            let header = read_record_header(file)?;
            let mut k_buffer = vec![0u8; header.key_size as usize];
            file.read_exact(&mut k_buffer)?;
            file.seek(SeekFrom::Current(header.value_size as i64))?;

            if header.tombstone {
                continue;
            }
            let key =
                String::from_utf8(k_buffer).map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
            hashmap.insert(key, record_offset);
        }

        Ok(HashIndex { hashmap })
    }
}
