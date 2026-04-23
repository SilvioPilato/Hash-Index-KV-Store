use std::ops::Bound::Included;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
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
use std::sync::atomic::Ordering::Relaxed;

struct LsmShared {
    active: RwLock<Memtable>,
    immutable: RwLock<Option<Memtable>>,
    flush_handle: Mutex<Option<thread::JoinHandle<()>>>,
    db_path: String,
    db_name: String,
    max_memtable_bytes: AtomicUsize,
    wal: Mutex<Wal>,
    storage_strategy: RwLock<Box<dyn StorageStrategy>>,
    block_size_bytes: usize,
    block_compression_enabled: bool,
}

pub struct LsmEngine {
    shared: Arc<LsmShared>,
}

impl LsmEngine {
    pub fn new(
        db_path: &str,
        db_name: &str,
        max_memtable_bytes: usize,
        storage_strategy: Box<dyn StorageStrategy>,
        block_size_bytes: usize,
        block_compression_enabled: bool,
    ) -> io::Result<LsmEngine> {
        let wal = Wal::open(&PathBuf::from(db_path), db_name.to_string())?;
        Ok(LsmEngine {
            shared: Arc::new(LsmShared {
                active: RwLock::new(Memtable::new()),
                immutable: RwLock::new(None),
                flush_handle: Mutex::new(None),
                db_path: db_path.to_string(),
                db_name: db_name.to_string(),
                max_memtable_bytes: AtomicUsize::from(max_memtable_bytes),
                wal: Mutex::new(wal),
                storage_strategy: RwLock::new(storage_strategy),
                block_size_bytes,
                block_compression_enabled,
            }),
        })
    }

    pub fn from_dir(
        dir: &str,
        db_name: &str,
        max_memtable_bytes: usize,
        storage_strategy: Box<dyn StorageStrategy>,
        block_size_bytes: usize,
        block_compression_enabled: bool,
    ) -> io::Result<Self> {
        let wal = Wal::open(&PathBuf::from(dir), db_name.to_string())?;
        let memtable = wal.replay()?;
        Ok(LsmEngine {
            shared: Arc::new(LsmShared {
                active: RwLock::new(memtable),
                immutable: RwLock::new(None),
                flush_handle: Mutex::new(None),
                db_path: dir.to_string(),
                db_name: db_name.to_string(),
                max_memtable_bytes: AtomicUsize::from(max_memtable_bytes),
                wal: Mutex::new(wal),
                storage_strategy: RwLock::new(storage_strategy),
                block_size_bytes,
                block_compression_enabled,
            }),
        })
    }

    fn flush_memtable_async(&self) -> io::Result<()> {
        // Hold flush_handle for the entire operation to serialize concurrent flush calls
        let mut handle = self.shared.flush_handle.lock().unwrap();

        // Backpressure: wait for any in-flight flush to finish
        if let Some(h) = handle.take() {
            h.join().unwrap();
        }

        // Previous flush is done — immutable is None, safe to swap
        {
            let mut wal = self.shared.wal.lock().unwrap();
            let mut active = self.shared.active.write().unwrap();
            let mut immutable = self.shared.immutable.write().unwrap();
            let old = std::mem::take(&mut *active);
            *immutable = Some(old);
            wal.reset().map_err(|e| {
                eprintln!("flush_memtable_async: WAL reset failed: {}", e);
                e
            })?;
        }

        let shared = Arc::clone(&self.shared);
        let jh = thread::spawn(move || {
            {
                let immutable = shared.immutable.read().unwrap();
                if let Some(ref memtable) = *immutable {
                    let sstable = SSTable::from_memtable(
                        &shared.db_path,
                        &shared.db_name,
                        memtable,
                        None,
                        shared.block_size_bytes,
                        shared.block_compression_enabled,
                    )
                    .unwrap_or_else(|e| {
                        eprintln!("flush_memtable_async: SSTable write failed: {}", e);
                        panic!("flush_memtable_async: SSTable write failed");
                    });
                    drop(immutable);
                    let mut storage_strategy = shared.storage_strategy.write().unwrap();
                    storage_strategy.add_sstable(sstable).unwrap_or_else(|e| {
                        eprintln!("flush_memtable_async: add_sstable failed: {}", e);
                        panic!("flush_memtable_async: add_sstable failed");
                    });
                }
            }

            {
                let mut immutable = shared.immutable.write().unwrap();
                *immutable = None;
            }
        });

        *handle = Some(jh);
        Ok(())
    }
}

impl Drop for LsmEngine {
    fn drop(&mut self) {
        let mut handle = self.shared.flush_handle.lock().unwrap();
        if let Some(h) = handle.take() {
            let _ = h.join();
        }
    }
}

impl StorageEngine for LsmEngine {
    fn get(&self, key: &str) -> Result<Option<(String, String)>, std::io::Error> {
        {
            let memtable = self.shared.active.read().unwrap();
            match memtable.entry(key) {
                Some(Some(v)) => return Ok(Some((key.to_string(), v.clone()))),
                Some(None) => return Ok(None),
                None => {}
            }
        }

        {
            let immutable = self.shared.immutable.read().unwrap();
            if let Some(memtable) = immutable.as_ref() {
                match memtable.entry(key) {
                    Some(Some(v)) => return Ok(Some((key.to_string(), v.clone()))),
                    Some(None) => return Ok(None),
                    None => {}
                }
            }
        }

        {
            let storage_strategy = self.shared.storage_strategy.read().unwrap();
            for segment in storage_strategy.iter_for_key(key) {
                match segment.get(key)? {
                    Some(Some(v)) => return Ok(Some((key.to_string(), v))),
                    Some(None) => return Ok(None),
                    None => continue,
                }
            }
        }

        Ok(None)
    }

    fn set(&self, key: &str, value: &str) -> Result<(), std::io::Error> {
        let memtable_size = {
            let mut wal = self.shared.wal.lock().unwrap();
            let mut memtable = self.shared.active.write().unwrap();
            wal.append(key.to_string(), value.to_string(), false)?;
            memtable.insert(key.to_string(), value.to_string());
            memtable.size_bytes()
        };

        if memtable_size >= self.shared.max_memtable_bytes.load(Relaxed) {
            self.flush_memtable_async()?;
        }

        Ok(())
    }

    fn delete(&self, key: &str) -> Result<Option<()>, std::io::Error> {
        let exists = self.get(key)?.is_some();

        let size_bytes = {
            let mut wal = self.shared.wal.lock().unwrap();
            let mut active = self.shared.active.write().unwrap();
            wal.append(key.to_string(), String::new(), true)?;
            active.remove(key.to_string());
            active.size_bytes()
        };

        if size_bytes >= self.shared.max_memtable_bytes.load(Relaxed) {
            self.flush_memtable_async()?;
        }

        if exists { Ok(Some(())) } else { Ok(None) }
    }

    fn compact(&self) -> Result<(), std::io::Error> {
        // Wait for any in-flight background flush to finish
        {
            let mut handle = self.shared.flush_handle.lock().unwrap();
            if let Some(h) = handle.take() {
                h.join().unwrap();
            }
        }

        let mut wal = self.shared.wal.lock().unwrap();
        let mut storage_strategy = self.shared.storage_strategy.write().unwrap();

        // Inline flush under storage_strategy lock to prevent concurrent
        // flush_memtable from sneaking an SSTable in before compact_all.
        {
            let mut active = self.shared.active.write().unwrap();
            let mut immutable = self.shared.immutable.write().unwrap();
            let old = std::mem::take(&mut *active);
            *immutable = Some(old);
        }
        {
            let immutable = self.shared.immutable.read().unwrap();
            if let Some(ref memtable) = *immutable {
                let sstable = SSTable::from_memtable(
                    &self.shared.db_path,
                    &self.shared.db_name,
                    memtable,
                    None,
                    self.shared.block_size_bytes,
                    self.shared.block_compression_enabled,
                )?;
                drop(immutable);
                storage_strategy.add_sstable(sstable)?;
            }
        }

        wal.reset()?;

        {
            let mut immutable = self.shared.immutable.write().unwrap();
            *immutable = None;
        }

        storage_strategy.compact_all(&self.shared.db_path, &self.shared.db_name)?;

        Ok(())
    }

    fn compact_step(&self) -> io::Result<bool> {
        let mut storage_strategy = self.shared.storage_strategy.write().unwrap();
        storage_strategy.compact_if_needed(&self.shared.db_path, &self.shared.db_name)
    }

    fn dead_bytes(&self) -> u64 {
        0
    }

    fn total_bytes(&self) -> u64 {
        0
    }

    fn segment_count(&self) -> usize {
        let storage_strategy = self.shared.storage_strategy.read().unwrap();
        storage_strategy.segment_count()
    }

    fn list_keys(&self) -> io::Result<Vec<String>> {
        let mut keys: HashSet<String> = HashSet::new();
        {
            let storage_strategy = self.shared.storage_strategy.read().unwrap();
            for segment in storage_strategy.iter_all() {
                for result in segment.iter()? {
                    let record = result?;
                    if record.header.tombstone {
                        keys.remove(&record.key);
                    } else {
                        keys.insert(record.key);
                    }
                }
            }
        }

        {
            let immutable = self.shared.immutable.read().unwrap();
            if let Some(memtable) = immutable.as_ref() {
                for (key, opt) in memtable.entries() {
                    if opt.is_some() {
                        keys.insert(key.clone());
                    } else {
                        keys.remove(key);
                    }
                }
            }
        }

        {
            let memtable = self.shared.active.read().unwrap();
            for (key, opt) in memtable.entries() {
                if opt.is_some() {
                    keys.insert(key.clone());
                } else {
                    keys.remove(key);
                }
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

    fn mset(&self, items: Vec<(String, String)>) -> Result<(), std::io::Error> {
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
        {
            let storage_strategy = self.shared.storage_strategy.read().unwrap();
            for segment in storage_strategy.iter_files_for_range(start, end) {
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
        }

        {
            let immutable = self.shared.immutable.read().unwrap();
            if let Some(memtable) = immutable.as_ref() {
                for (k, v) in memtable
                    .entries()
                    .range::<str, _>((Included(start), Included(end)))
                {
                    match v {
                        Some(val) => b_map.insert(k.clone(), val.clone()),
                        None => b_map.remove(k),
                    };
                }
            }
        }

        {
            let memtable = self.shared.active.read().unwrap();
            for (k, v) in memtable
                .entries()
                .range::<str, _>((Included(start), Included(end)))
            {
                match v {
                    Some(val) => b_map.insert(k.clone(), val.clone()),
                    None => b_map.remove(k),
                };
            }
        }

        Ok(b_map.into_iter().collect())
    }
}
