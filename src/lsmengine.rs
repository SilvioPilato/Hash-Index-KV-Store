use std::{fs, io};

use crate::{
    engine::StorageEngine,
    memtable::Memtable,
    sstable::{SSTable, get_sstables},
};

pub struct LsmEngine {
    memtable: Memtable,
    segments: Vec<SSTable>, // sorted oldest-to-newest by timestamp
    db_path: String,
    db_name: String,
    max_memtable_bytes: usize, // flush threshold
}

impl LsmEngine {
    pub fn new(db_path: &str, db_name: &str, max_memtable_bytes: usize) -> Self {
        LsmEngine {
            memtable: Memtable::new(),
            segments: Vec::new(),
            db_path: db_path.to_string(),
            db_name: db_name.to_string(),
            max_memtable_bytes,
        }
    }

    pub fn from_dir(dir: &str, db_name: &str, max_memtable_bytes: usize) -> io::Result<Self> {
        Ok(LsmEngine {
            memtable: Memtable::new(),
            segments: get_sstables(dir, db_name)?,
            db_path: dir.to_string(),
            db_name: db_name.to_string(),
            max_memtable_bytes,
        })
    }
}

impl StorageEngine for LsmEngine {
    fn get(&self, key: &str) -> Result<Option<(String, String)>, std::io::Error> {
        // 1. Check memtable
        match self.memtable.entry(key) {
            Some(Some(v)) => return Ok(Some((key.to_string(), v.clone()))),
            Some(None) => return Ok(None), // tombstone — stop searching
            None => {}                     // not in memtable, check segments
        }
        // 2. Check segments newest-to-oldest
        for segment in self.segments.iter().rev() {
            match segment.get(key)? {
                Some(Some(v)) => return Ok(Some((key.to_string(), v))),
                Some(None) => return Ok(None), // tombstone
                None => continue,              // not in this segment
            }
        }

        // 3. Not found anywhere
        Ok(None)
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), std::io::Error> {
        self.memtable.insert(key.to_string(), value.to_string());

        if self.memtable.size_bytes() >= self.max_memtable_bytes {
            let sstable = SSTable::from_memtable(&self.db_path, &self.db_name, &self.memtable)?;
            self.segments.push(sstable);
            self.memtable.clear();
        }

        Ok(())
    }

    fn delete(&mut self, key: &str) -> Result<Option<()>, std::io::Error> {
        self.memtable.remove(key.to_string());

        if self.memtable.size_bytes() >= self.max_memtable_bytes {
            let sstable = SSTable::from_memtable(&self.db_path, &self.db_name, &self.memtable)?;
            self.segments.push(sstable);
            self.memtable.clear();
        }

        Ok(Some(()))
    }

    fn compact(&mut self) -> Result<(), std::io::Error> {
        let mut memtable = Memtable::new();
        for segment in self.segments.iter() {
            for result in segment.iter()? {
                let record = result?;
                if record.header.tombstone {
                    memtable.remove(record.key);
                } else {
                    memtable.insert(record.key, record.value);
                }
            }
        }

        for (key, opt) in self.memtable.entries() {
            match opt.to_owned() {
                Some(value) => {
                    memtable.insert(key.to_string(), value);
                }
                None => {
                    memtable.remove(key.to_string());
                }
            }
        }
        memtable.drop_tombstones();

        // Delete old segment files before creating the new one to avoid
        // timestamp collisions (same millisecond) where from_memtable would
        // overwrite an old file that then gets deleted.
        for segment in self.segments.iter() {
            fs::remove_file(&segment.path)?;
        }

        let new_sstable = SSTable::from_memtable(&self.db_path, &self.db_name, &memtable)?;

        self.segments = vec![new_sstable];
        self.memtable.clear();
        Ok(())
    }
}
