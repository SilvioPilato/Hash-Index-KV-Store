use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Mutex;

use crate::hash_index::HashIndex;
use crate::utils;

pub struct DB {
    index: HashIndex,
    db_file: Mutex<File>,
    db_file_path: String,
}

impl DB {
    pub fn new(db_file_path: &str) -> DB {
        let f_name = utils::get_new_db_file_name(db_file_path).unwrap();
        let file: File = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .append(true)
            .open(f_name)
            .unwrap();

        DB {
            index: HashIndex::new(),
            db_file: Mutex::new(file),
            db_file_path: db_file_path.to_string(),
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let offset = self.index.get(key)?;

        let mut file = self.db_file.lock().unwrap();
        let mut size_buffer = [0; 8];
        file.seek(SeekFrom::Start(*offset)).unwrap();
        file.read_exact(&mut size_buffer).unwrap();
        let size = u64::from_be_bytes(size_buffer);

        let v_offset = offset + 8;
        let mut str_buffer = vec![0; size as usize];
        file.seek(SeekFrom::Start(v_offset)).unwrap();
        file.read_exact(&mut str_buffer).unwrap();

        match String::from_utf8(str_buffer) {
            Ok(s) => Some(s),
            Err(_) => None,
        }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        let mut file = self.db_file.lock().unwrap();
        let current_eof_offset = file.seek(SeekFrom::End(0)).unwrap();
        let value_bytes = value.as_bytes();
        let value_size: u64 = value_bytes.len() as u64;

        file.write_all(&value_size.to_be_bytes()).unwrap();
        file.write_all(value_bytes).unwrap();

        self.index.set(key.to_string(), current_eof_offset);
        file.flush().unwrap();
    }

    pub fn delete(&mut self, key: &str) {
        self.index.delete(&key);
    }

    pub fn get_compacted(&self) -> DB {
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
                continue;
            };
            new_db.set(&k, &value);
        }

        return new_db;
    }
}

#[cfg(test)]
mod tests {
    use super::DB;
    use std::{env, time::SystemTime};

    fn temp_db_path(suffix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut path = env::temp_dir();
        path.push(format!("kv_store_{}_{}", suffix, nanos));
        path.to_string_lossy().to_string()
    }

    #[test]
    fn compact_preserves_values() {
        let mut db = DB::new(&temp_db_path("preserve"));
        db.set("k1", "v1");
        db.set("k2", "v2");

        let compacted = db.get_compacted();

        assert_eq!(compacted.get("k1").as_deref(), Some("v1"));
        assert_eq!(compacted.get("k2").as_deref(), Some("v2"));
    }

    #[test]
    fn compact_keeps_latest_value() {
        let mut db = DB::new(&temp_db_path("latest"));
        db.set("k1", "v1");
        db.set("k1", "v2");

        let compacted = db.get_compacted();

        assert_eq!(compacted.get("k1").as_deref(), Some("v2"));
    }

    #[test]
    fn compact_drops_deleted_keys() {
        let mut db = DB::new(&temp_db_path("deleted"));
        db.set("k1", "v1");
        db.delete("k1");

        let compacted = db.get_compacted();

        assert_eq!(compacted.get("k1"), None);
    }

    #[test]
    fn compact_is_idempotent() {
        let mut db = DB::new(&temp_db_path("idempotent"));
        db.set("k1", "v1");
        db.set("k2", "v2");

        let compacted = db.get_compacted();
        let compacted_again = compacted.get_compacted();

        assert_eq!(compacted_again.get("k1").as_deref(), Some("v1"));
        assert_eq!(compacted_again.get("k2").as_deref(), Some("v2"));
    }
}
