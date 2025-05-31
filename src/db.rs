use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};

use crate::hash_index::HashIndex;
use crate::utils;

pub struct DB {
    index: HashIndex,
    db_file: File,
    db_file_path: String
}

impl DB {
    pub fn new(db_file_path: &str) -> DB {
        let f_name = utils::get_new_db_file_name(db_file_path).unwrap();
        let file: File = OpenOptions::new()
            .read(true)
            .write(true) 
            .create(true)
            .append(true)
            .open(f_name).unwrap();

        DB {
            index: HashIndex::new(),
            db_file: file,
            db_file_path: db_file_path.to_string()
        }
    }

    pub fn get(&mut self, key: &str) -> Option<String>  {
        let offset = self.index.get(key)?;

        let mut size_buffer = [0; 8];
        self.db_file.seek(SeekFrom::Start(*offset)).unwrap();
        self.db_file.read_exact(&mut size_buffer).unwrap();
        let size = u64::from_be_bytes(size_buffer);

        let v_offset = offset + 8;
        let mut str_buffer = vec![0; size as usize];
        self.db_file.seek(SeekFrom::Start(v_offset)).unwrap();
        self.db_file.read_exact(&mut str_buffer).unwrap();

        match String::from_utf8(str_buffer) {
            Ok(s) => Some(s),
            Err(_) => None,
        }
    }

    pub fn set(&mut self, key: &str, value: &str)  {
        let current_eof_offset = self.db_file.seek(SeekFrom::End(0)).unwrap();
        let value_bytes = value.as_bytes();
        let value_size: u64 = value_bytes.len() as u64;

        self.db_file.write_all(&value_size.to_be_bytes()).unwrap();
        self.db_file.write_all(value_bytes).unwrap();

        self.index.set(key.to_string(), current_eof_offset);
        self.db_file.flush().unwrap();
    }

    pub fn delete(&mut self, key: &str) {
        self.index.delete(&key);
    }

    pub fn get_compacted(&mut self) -> DB {
        // make a new DB
        // make a new HashIndex
        // deduplicate data using the current db pouring data into the new one
        // return the new DB

        let mut new_db = DB::new(&self.db_file_path);

        let keys: Vec<String> = self.index.ls_keys().cloned().collect();
        for k in keys {
            let value = if let Some(value) = self.get(&k) {
                value
            } else {
                continue
            };
            new_db.set(&k, &value);
        };

        return new_db
    }
 }