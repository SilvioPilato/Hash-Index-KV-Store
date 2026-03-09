use std::fs::{File, OpenOptions};
use std::io::{Error, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

use crate::hash_index::HashIndex;
use crate::utils::{self, Record, read_record, read_record_header};

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
            .open(f_name)
            .unwrap();

        DB {
            index: HashIndex::new(),
            db_file: Mutex::new(file),
            db_file_path: db_file_path.to_string(),
        }
    }

    pub fn from_file(db_file_path: &str) -> Result<Option<DB>, Error> {
        if !Path::new(db_file_path).exists() {
            return Ok(None);
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(db_file_path)?;

        // open
        Ok(Some(DB {
            index: HashIndex::from_file(&mut file)?,
            db_file: Mutex::new(file),
            db_file_path: db_file_path.to_string(),
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
        let mut file = self.db_file.lock().unwrap();
        let current_eof_offset = file.seek(SeekFrom::End(0))?;
        let key_bytes = key.as_bytes();
        let key_size = key.len() as u64;
        let value_bytes = value.as_bytes();
        let value_size: u64 = value_bytes.len() as u64;
        let tombstone_marker: u8 = 0;
        let mut buf = Vec::with_capacity(17 + key_bytes.len() + value_bytes.len());
        buf.extend_from_slice(&key_size.to_be_bytes());
        buf.extend_from_slice(&value_size.to_be_bytes());
        buf.extend_from_slice(&tombstone_marker.to_be_bytes());
        buf.extend_from_slice(key_bytes);
        buf.extend_from_slice(value_bytes);
        file.write_all(&buf)?;

        self.index.set(key.to_string(), current_eof_offset);
        file.flush()?;
        Ok(())
    }

    pub fn delete(&mut self, key: &str) -> Result<Option<()>, Error> {
        let mut file = self.db_file.lock().unwrap();
        match self.index.delete(&key) {
            Some(offset) => {
                let tombstone_buf = &[1u8];
                file.seek(SeekFrom::Start(offset + 8 + 8))?;
                file.write_all(tombstone_buf)?;
                file.flush()?;
                Ok(Some(()))
            }
            None => Ok(None),
        }
    }

    pub fn get_compacted(&self) -> Result<DB, Error> {
        let mut new_db = DB::new(&self.db_file_path);

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
    fn set_and_get() {
        let mut db = DB::new(&temp_db_path("set_get"));
        db.set("hello", "world").unwrap();
        let (_, value) = db.get("hello").unwrap().unwrap();
        assert_eq!(value, "world");
    }

    #[test]
    fn get_missing_key() {
        let db = DB::new(&temp_db_path("missing"));
        assert_eq!(db.get("nope").unwrap(), None);
    }

    #[test]
    fn set_overwrite() {
        let mut db = DB::new(&temp_db_path("overwrite"));
        db.set("k", "old").unwrap();
        db.set("k", "new").unwrap();
        let (_, value) = db.get("k").unwrap().unwrap();
        assert_eq!(value, "new");
    }

    #[test]
    fn compact_preserves_values() {
        let mut db = DB::new(&temp_db_path("preserve"));
        db.set("k1", "v1").unwrap();
        db.set("k2", "v2").unwrap();

        let compacted = db.get_compacted().unwrap();

        let (_, v1) = compacted.get("k1").unwrap().unwrap();
        let (_, v2) = compacted.get("k2").unwrap().unwrap();
        assert_eq!(v1, "v1");
        assert_eq!(v2, "v2");
    }

    #[test]
    fn compact_keeps_latest_value() {
        let mut db = DB::new(&temp_db_path("latest"));
        db.set("k1", "v1").unwrap();
        db.set("k1", "v2").unwrap();

        let compacted = db.get_compacted().unwrap();

        let (_, value) = compacted.get("k1").unwrap().unwrap();
        assert_eq!(value, "v2");
    }

    #[test]
    fn compact_drops_deleted_keys() {
        let mut db = DB::new(&temp_db_path("deleted"));
        db.set("k1", "v1").unwrap();
        db.delete("k1");

        let compacted = db.get_compacted().unwrap();

        assert_eq!(compacted.get("k1").unwrap(), None);
    }

    #[test]
    fn compact_is_idempotent() {
        let mut db = DB::new(&temp_db_path("idempotent"));
        db.set("k1", "v1").unwrap();
        db.set("k2", "v2").unwrap();

        let compacted = db.get_compacted().unwrap();
        let compacted_again = compacted.get_compacted().unwrap();

        let (_, v1) = compacted_again.get("k1").unwrap().unwrap();
        let (_, v2) = compacted_again.get("k2").unwrap().unwrap();
        assert_eq!(v1, "v1");
        assert_eq!(v2, "v2");
    }
}
