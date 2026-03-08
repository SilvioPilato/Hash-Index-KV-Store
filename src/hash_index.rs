use std::{
    collections::HashMap,
    fs::File,
    io::{Error, ErrorKind, Read, Seek, SeekFrom},
};

use crate::utils::read_record_header;

pub struct HashIndex {
    hashmap: HashMap<String, u64>,
}

impl HashIndex {
    pub(crate) fn new() -> HashIndex {
        let hashmap = HashMap::new();
        return HashIndex { hashmap };
    }

    pub fn get(&self, key: &str) -> Option<&u64> {
        self.hashmap.get(key)
    }

    pub fn set(&mut self, key: String, location: u64) {
        self.hashmap.insert(key, location);
    }

    pub fn ls_keys(&self) -> std::collections::hash_map::Keys<'_, String, u64> {
        self.hashmap.keys()
    }

    pub fn delete(&mut self, key: &str) -> Option<u64> {
        self.hashmap.remove(key)
    }

    pub fn from_file(file: &mut File) -> Result<HashIndex, Error> {
        let mut hashmap = HashMap::new();
        let file_size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;

        while file.seek(SeekFrom::Current(0))? < file_size {
            let record_offset = file.seek(SeekFrom::Current(0))?;
            let header = read_record_header(file)?;

            let mut k_buffer = vec![0u8; header.key_size as usize];
            file.read_exact(&mut k_buffer)?;

            // Skip past the value bytes
            file.seek(SeekFrom::Current(header.value_size as i64))?;

            let key =
                String::from_utf8(k_buffer).map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
            hashmap.insert(key, record_offset);
        }

        Ok(HashIndex { hashmap })
    }
}
