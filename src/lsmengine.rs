use std::ops::Bound::Included;
use std::{
    any::Any,
    collections::{BTreeMap, HashSet},
    io,
    path::PathBuf,
};

use crate::storage_strategy::StorageStrategy;
use crate::{
    engine::{RangeScan, StorageEngine},
    memtable::Memtable,
    sstable::SSTable,
    wal::Wal,
};

pub struct LsmEngine {
    memtable: Memtable,
    db_path: String,
    db_name: String,
    max_memtable_bytes: usize, // flush threshold
    wal: Wal,
    storage_strategy: Box<dyn StorageStrategy>,
}

impl LsmEngine {
    pub fn new(
        db_path: &str,
        db_name: &str,
        max_memtable_bytes: usize,
        storage_strategy: Box<dyn StorageStrategy>,
    ) -> io::Result<LsmEngine> {
        let wal = Wal::open(&PathBuf::from(db_path), db_name.to_string())?;
        Ok(LsmEngine {
            memtable: Memtable::new(),
            db_path: db_path.to_string(),
            db_name: db_name.to_string(),
            max_memtable_bytes,
            wal,
            storage_strategy,
        })
    }

    pub fn from_dir(
        dir: &str,
        db_name: &str,
        max_memtable_bytes: usize,
        storage_strategy: Box<dyn StorageStrategy>,
    ) -> io::Result<Self> {
        let wal = Wal::open(&PathBuf::from(dir), db_name.to_string())?;
        let memtable = wal.replay()?;
        Ok(LsmEngine {
            memtable,
            db_path: dir.to_string(),
            db_name: db_name.to_string(),
            max_memtable_bytes,
            wal,
            storage_strategy,
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
        for segment in self.storage_strategy.iter_for_key(key) {
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
            self.storage_strategy.add_sstable(SSTable::from_memtable(
                &self.db_path,
                &self.db_name,
                &self.memtable,
            )?)?;
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
            self.storage_strategy.add_sstable(SSTable::from_memtable(
                &self.db_path,
                &self.db_name,
                &self.memtable,
            )?)?;
            self.memtable.clear();
            self.wal.reset()?;
        }

        if exists { Ok(Some(())) } else { Ok(None) }
    }

    fn compact(&mut self) -> Result<(), std::io::Error> {
        self.storage_strategy.add_sstable(SSTable::from_memtable(
            &self.db_path,
            &self.db_name,
            &self.memtable,
        )?)?;
        self.memtable.clear();
        self.storage_strategy
            .compact_all(&self.db_path, &self.db_name)?;
        Ok(())
    }

    fn compact_step(&mut self) -> io::Result<bool> {
        self.storage_strategy
            .compact_if_needed(&self.db_path, &self.db_name)
    }

    fn dead_bytes(&self) -> u64 {
        0
    }

    fn total_bytes(&self) -> u64 {
        0
    }

    fn segment_count(&self) -> usize {
        self.storage_strategy.segment_count()
    }

    fn list_keys(&self) -> io::Result<Vec<String>> {
        let mut keys: HashSet<String> = HashSet::new();

        for segment in self.storage_strategy.iter_all() {
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

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl RangeScan for LsmEngine {
    fn range(&self, start: &str, end: &str) -> io::Result<Vec<(String, String)>> {
        if start > end {
            return Ok(vec![]);
        }
        let mut b_map: BTreeMap<String, String> = BTreeMap::new();
        for segment in self.storage_strategy.iter_files_for_range(start, end) {
            for result in segment.iter()? {
                let record = result?;
                if record.key.as_str() < start || record.key.as_str() > end {
                    continue;
                }
                if record.header.tombstone {
                    b_map.remove(&record.key);
                    continue;
                }

                b_map.insert(record.key, record.value);
            }
        }

        for (k, v) in self
            .memtable
            .entries()
            .range::<str, _>((Included(start), Included(end)))
        {
            match v {
                Some(val) => b_map.insert(k.clone(), val.clone()),
                None => b_map.remove(k),
            };
        }
        Ok(b_map.into_iter().collect())
    }
}
