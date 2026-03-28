use std::{collections::HashSet, fs, io, path::PathBuf};

use crate::{
    engine::StorageEngine,
    memtable::Memtable,
    sstable::{SSTable, get_sstables},
    wal::Wal,
};

pub struct LsmEngine {
    memtable: Memtable,
    segments: Vec<SSTable>, // sorted oldest-to-newest by timestamp
    db_path: String,
    db_name: String,
    max_memtable_bytes: usize, // flush threshold
    wal: Wal,
}

impl LsmEngine {
    pub fn new(db_path: &str, db_name: &str, max_memtable_bytes: usize) -> io::Result<LsmEngine> {
        let wal = Wal::open(&PathBuf::from(db_path), db_name.to_string())?;
        Ok(LsmEngine {
            memtable: Memtable::new(),
            segments: Vec::new(),
            db_path: db_path.to_string(),
            db_name: db_name.to_string(),
            max_memtable_bytes,
            wal,
        })
    }

    pub fn from_dir(dir: &str, db_name: &str, max_memtable_bytes: usize) -> io::Result<Self> {
        let wal = Wal::open(&PathBuf::from(dir), db_name.to_string())?;
        let memtable = wal.replay()?;
        Ok(LsmEngine {
            memtable,
            segments: get_sstables(dir, db_name)?,
            db_path: dir.to_string(),
            db_name: db_name.to_string(),
            max_memtable_bytes,
            wal,
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
        self.wal.append(key.to_string(), value.to_string(), false)?;
        self.memtable.insert(key.to_string(), value.to_string());

        if self.memtable.size_bytes() >= self.max_memtable_bytes {
            let sstable = SSTable::from_memtable(&self.db_path, &self.db_name, &self.memtable)?;
            self.segments.push(sstable);
            self.memtable.clear();
            self.wal.reset()?;
        }

        Ok(())
    }

    fn delete(&mut self, key: &str) -> Result<Option<()>, std::io::Error> {
        // Check if key exists
        let exists = self.get(key)?.is_some();
        self.wal.append(key.to_string(), String::new(), true)?;

        self.memtable.remove(key.to_string());

        if self.memtable.size_bytes() >= self.max_memtable_bytes {
            let sstable = SSTable::from_memtable(&self.db_path, &self.db_name, &self.memtable)?;
            self.segments.push(sstable);
            self.memtable.clear();
            self.wal.reset()?;
        }

        if exists { Ok(Some(())) } else { Ok(None) }
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

    fn dead_bytes(&self) -> u64 {
        0
    }

    fn total_bytes(&self) -> u64 {
        0
    }

    fn segment_count(&self) -> usize {
        self.segments.len()
    }

    fn list_keys(&self) -> io::Result<Vec<String>> {
        let mut keys: HashSet<String> = HashSet::new();

        for segment in self.segments.iter() {
            for result in segment.iter()? {
                let record = result?;
                if record.header.tombstone {
                    keys.remove(&record.key);
                } else {
                    keys.insert(record.key);
                }
            }
        }

        for (key, opt) in self.memtable.entries() {
            if opt.is_some() {
                keys.insert(key.clone());
            } else {
                keys.remove(key);
            }
        }

        Ok(keys.into_iter().collect())
    }

    fn exists(&self, key: &str) -> bool {
        self.get(key).map(|v| v.is_some()).unwrap_or(false)
    }

    fn mget(&self, keys: Vec<String>) -> Result<Vec<(String, Option<String>)>, std::io::Error> {
        let mut res: Vec<(String, Option<String>)> = Vec::new();
        for key in keys {
            match self.get(&key)? {
                Some((k, v)) => {
                    res.push((k, Some(v)));
                }
                None => {
                    res.push((key, None));
                }
            }
        }

        Ok(res)
    }

    fn mset(&mut self, items: Vec<(String, String)>) -> Result<(), std::io::Error> {
        for (k, v) in items {
            self.set(&k, &v)?;
        }

        Ok(())
    }
}
