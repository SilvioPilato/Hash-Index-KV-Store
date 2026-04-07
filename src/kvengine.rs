use std::any::Any;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Error, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Mutex, RwLock};

use crate::engine::StorageEngine;
use crate::hash_index::HashIndex;
use crate::hint::{Hint, HintEntry};
use crate::record::{MAX_KEY_SIZE, MAX_VALUE_SIZE, Record, RecordHeader};
use crate::segment::{Segment, get_segments};
use crate::settings::FSyncStrategy;
use crate::wal::Wal;
use crate::worker::BackgroundWorker;

pub struct KVEngine {
    index: RwLock<HashIndex>,
    wal: Mutex<Wal>,
    active_file: Mutex<ActiveFileState>,
    active_segment: Mutex<Segment>,
    db_path: String,
    db_name: String,
    max_segment_bytes: AtomicU64,
    writes_since_fsync: AtomicU64,
    fsync_strategy: FSyncStrategy,
    dead_bytes: AtomicU64,
    total_bytes: AtomicU64,
    segment_count: AtomicUsize,
}

struct ActiveFileState {
    file: File,
    fsync_handle: Option<BackgroundWorker>,
}

impl KVEngine {
    /// Creates a new, empty database in the given directory.
    ///
    /// Creates `db_path` if it does not exist, opens a fresh segment file,
    /// and returns a `DB` ready for reads and writes.
    pub fn new(
        db_path: &str,
        db_name: &str,
        max_segment_bytes: u64,
        fsync_strategy: FSyncStrategy,
    ) -> io::Result<KVEngine> {
        std::fs::create_dir_all(db_path)?;
        let segment = Segment::new(db_name).map_err(io::Error::other)?;
        let file: File = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(segment.path(db_path))?;
        let fsync_handle = Self::spawn_fsync_worker(fsync_strategy, segment.path(db_path));
        Ok(KVEngine {
            index: RwLock::new(HashIndex::new()),
            active_file: Mutex::new(ActiveFileState { file, fsync_handle }),
            db_path: db_path.to_string(),
            db_name: db_name.to_string(),
            active_segment: Mutex::new(segment),
            max_segment_bytes: AtomicU64::from(max_segment_bytes),
            writes_since_fsync: AtomicU64::from(0),
            fsync_strategy,
            dead_bytes: AtomicU64::from(0),
            total_bytes: AtomicU64::from(0),
            segment_count: AtomicUsize::from(1),
            wal: Mutex::new(Wal::open(&PathBuf::from(db_path), db_name.to_string())?),
        })
    }

    /// Reopens an existing database from disk.
    ///
    /// Scans `db_dir` for segment files matching `db_name`, rebuilds the
    /// in-memory index (from hint files when available, otherwise by scanning
    /// records), and returns a `DB` positioned at the latest segment.
    ///
    /// Returns `Ok(None)` if the directory does not exist or contains no
    /// matching segments.
    pub fn from_dir(
        db_dir: &str,
        db_name: &str,
        max_segment_bytes: u64,
        fsync_strategy: FSyncStrategy,
    ) -> Result<Option<KVEngine>, Error> {
        let segments = match get_segments(db_dir, db_name) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };

        if segments.is_empty() {
            return Ok(None);
        }

        let mut hash_index = HashIndex::new();
        let mut current_file = None;
        let mut active_segment = None;
        let mut size = 0;

        for segment in segments.iter() {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(segment.path(db_dir))?;
            match Hint::read_file(segment.hint_path(db_dir)) {
                Ok(hints) => {
                    hints.iter().for_each(|entry| {
                        if !entry.tombstone {
                            hash_index.set(entry.key.clone(), entry.offset, segment.timestamp, 0);
                        }
                    });
                }
                Err(_) => {
                    hash_index.merge_from_file(&mut file, segment.timestamp)?;
                }
            }
            current_file = Some(file);
            active_segment = Some(segment);
            size += std::fs::metadata(segment.path(db_dir))?.len();
        }
        let mut wal = Wal::open(&PathBuf::from(db_dir), db_name.to_string())?;
        let memtable = wal.replay()?;
        let file = current_file.as_mut().unwrap();
        let segment = active_segment.unwrap();
        for (key, value) in memtable.entries() {
            match value {
                Some(val) => {
                    let record = Record {
                        header: RecordHeader {
                            crc32: 0u32,
                            key_size: key.len() as u64,
                            value_size: val.len() as u64,
                            tombstone: false,
                        },
                        key: key.to_string(),
                        value: val.to_string(),
                    };
                    let offset = record.append(file)?;
                    hash_index.set(
                        key.to_string(),
                        offset,
                        segment.timestamp,
                        record.size_on_disk(),
                    );
                    size += record.size_on_disk();
                }
                None => {
                    let record = Record {
                        header: RecordHeader {
                            crc32: 0u32,
                            key_size: key.len() as u64,
                            value_size: 0,
                            tombstone: true,
                        },
                        key: key.to_string(),
                        value: String::new(),
                    };
                    record.append(file)?;
                    hash_index.delete(key);
                    size += record.size_on_disk();
                }
            }
        }

        wal.reset()?;
        let segment = active_segment.unwrap().to_owned();
        let fsync_handle = Self::spawn_fsync_worker(fsync_strategy, segment.path(db_dir));
        Ok(Some(KVEngine {
            index: RwLock::new(hash_index),
            active_file: Mutex::new(ActiveFileState {
                file: current_file.unwrap(),
                fsync_handle,
            }),
            db_path: db_dir.to_string(),
            db_name: db_name.to_string(),
            active_segment: Mutex::new(Segment {
                segment_name: segment.segment_name,
                timestamp: segment.timestamp,
            }),
            max_segment_bytes: AtomicU64::from(max_segment_bytes),
            writes_since_fsync: AtomicU64::from(0),
            fsync_strategy,
            dead_bytes: AtomicU64::from(0),
            total_bytes: AtomicU64::from(size),
            segment_count: AtomicUsize::from(segments.len()),
            wal: Mutex::new(wal),
        }))
    }

    /// Builds a compacted KVEngine from the current state, returning
    /// the new engine without modifying self.
    fn build_compacted(&self) -> Result<(KVEngine, Vec<Segment>), Error> {
        let old_segments = get_segments(&self.db_path, &self.db_name)?;
        let new_db = KVEngine::new(
            &self.db_path,
            &self.db_name,
            self.max_segment_bytes.load(Relaxed),
            self.fsync_strategy,
        )?;
        let keys: Vec<String> = {
            let index_lock = self.index.read().unwrap();
            index_lock.ls_keys().cloned().collect()
        };

        for k in keys {
            let value = match self.get(&k)? {
                Some((_, value)) => value,
                None => continue,
            };
            new_db.set(&k, &value)?;
        }

        {
            let file_lock = new_db.active_file.lock().unwrap();
            file_lock.file.sync_all()?;
        }

        // Write hint files for the new segments (filter out old ones).
        {
            let new_index_lock = new_db.index.read().unwrap();
            let new_segments: Vec<_> = get_segments(&self.db_path, &self.db_name)?
                .into_iter()
                .filter(|s| !old_segments.iter().any(|o| o.timestamp == s.timestamp))
                .collect();
            for segment in &new_segments {
                let hint_entries: Vec<HintEntry> = new_index_lock
                    .ls_keys()
                    .filter_map(|k| {
                        let entry = new_index_lock.get(k).unwrap();
                        if entry.segment_timestamp == segment.timestamp {
                            Some(HintEntry {
                                key_size: k.len() as u64,
                                offset: entry.offset,
                                tombstone: false,
                                key: k.clone(),
                            })
                        } else {
                            None
                        }
                    })
                    .collect();
                Hint::write_file(segment.hint_path(&self.db_path), &hint_entries)?;
            }
        }

        // Don't delete old segments here — compact() deletes them
        // after swapping the index, so concurrent readers are safe.
        Ok((new_db, old_segments))
    }

    /// Closes the current active segment and opens a new one.
    /// Caller must hold both locks and pass them in.
    fn roll_segment(
        &self,
        active_file_lock: &mut ActiveFileState,
        active_segment_lock: &mut Segment,
    ) -> io::Result<()> {
        // Ensure old segment is durable before resetting WAL
        active_file_lock.file.sync_all()?;
        active_file_lock.fsync_handle.take();

        let segment = Segment::new(&self.db_name).map_err(io::Error::other)?;
        let file: File = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(segment.path(&self.db_path))?;

        let segment_path = segment.path(&self.db_path);
        *active_file_lock = ActiveFileState {
            file,
            fsync_handle: Self::spawn_fsync_worker(self.fsync_strategy, segment_path),
        };
        *active_segment_lock = segment;

        {
            let mut wal = self.wal.lock().unwrap();
            wal.reset()?
        }

        self.segment_count.fetch_add(1, Relaxed);
        Ok(())
    }

    /// Spawns a background worker that periodically fsyncs the given segment file.
    ///
    /// Returns `None` if the fsync strategy is not `Periodic`.
    fn spawn_fsync_worker(
        fsync_strategy: FSyncStrategy,
        segment_path: PathBuf,
    ) -> Option<BackgroundWorker> {
        if let FSyncStrategy::Periodic(duration) = fsync_strategy {
            let job = move || {
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&segment_path)
                    .unwrap();
                file.sync_all().unwrap();
            };
            Some(BackgroundWorker::spawn(duration, job))
        } else {
            None
        }
    }

    /// Flushes writes to disk according to the configured `FSyncStrategy`.
    fn fsync(&self) -> io::Result<()> {
        self.writes_since_fsync.fetch_add(1, Relaxed);
        match self.fsync_strategy {
            FSyncStrategy::Always => {
                self.writes_since_fsync.store(0, Relaxed);
                self.active_file.lock().unwrap().file.sync_all()?
            }
            FSyncStrategy::Never => {}
            FSyncStrategy::EveryN(n) => {
                if n <= self.writes_since_fsync.load(Relaxed) as usize {
                    self.writes_since_fsync.store(0, Relaxed);
                    self.active_file.lock().unwrap().file.sync_all()?
                }
            }
            FSyncStrategy::Periodic(_) => {}
        }

        Ok(())
    }
}

impl StorageEngine for KVEngine {
    /// Compacts the database by rewriting only live key-value pairs into
    /// fresh segments, then deleting the old segment and hint files.
    fn compact(&self) -> Result<(), Error> {
        let (new_db, old_segments) = self.build_compacted()?;

        let mut wal = self.wal.lock().unwrap();
        let mut file_state = self.active_file.lock().unwrap();
        let mut segment_guard = self.active_segment.lock().unwrap();
        let mut index_guard = self.index.write().unwrap();

        *file_state = new_db.active_file.into_inner().unwrap();
        *segment_guard = new_db.active_segment.into_inner().unwrap();
        *index_guard = new_db.index.into_inner().unwrap();

        self.writes_since_fsync.store(0, Relaxed);
        self.segment_count.store(1, Relaxed);

        wal.reset()?;

        // Drop all locks before file deletion — the index now points
        // to new segments, so no reader will reference old files.
        drop(index_guard);
        drop(segment_guard);
        drop(file_state);
        drop(wal);

        for segment in &old_segments {
            fs::remove_file(segment.path(&self.db_path))?;
            let _ = fs::remove_file(segment.hint_path(&self.db_path));
        }

        Ok(())
    }

    /// Retrieves the key-value pair associated with the given key.
    ///
    /// Looks up the key in the in-memory index, seeks to the record's byte
    /// offset in the appropriate segment file, and reads the full record
    /// (verifying its CRC32 checksum).
    ///
    /// Returns `Ok(None)` if the key is not in the index.
    fn get(&self, key: &str) -> Result<Option<(String, String)>, Error> {
        let active_segment = self.active_segment.lock().unwrap();
        let index = self.index.read().unwrap();
        let entry = match index.get(key) {
            Some(o) => o,
            None => return Ok(None),
        };

        let segment_offset = entry.offset;
        let segment_timestamp = entry.segment_timestamp;
        let active_timestamp = active_segment.timestamp;
        let active_path = active_segment.path(&self.db_path);
        drop(active_segment);

        // Hold index read lock across file I/O so compact() cannot
        // swap the index and delete old segment files while we read.
        let mut file = if segment_timestamp == active_timestamp {
            File::open(active_path)?
        } else {
            let segment = Segment {
                segment_name: self.db_name.clone(),
                timestamp: segment_timestamp,
            };
            File::open(segment.path(&self.db_path))?
        };
        drop(index);
        file.seek(SeekFrom::Start(segment_offset))?;
        let record = Record::read_next(&mut file)?;

        Ok(Some((record.key, record.value)))
    }

    /// Inserts or updates a key-value pair in the database.
    ///
    /// Appends a record to the active segment file and updates the in-memory
    /// index. If the active segment would exceed `max_segment_bytes`, a new
    /// segment is rolled first.
    ///
    /// Previous entries for the same key become dead bytes, reclaimable by
    /// compaction.
    fn set(&self, key: &str, value: &str) -> Result<(), Error> {
        if key.len() > MAX_KEY_SIZE || value.len() > MAX_VALUE_SIZE {
            return Err(Error::new(
                std::io::ErrorKind::InvalidInput,
                "key or value exceeds maximum allowed size",
            ));
        }

        {
            let mut wal = self.wal.lock().unwrap();
            wal.append(key.to_string(), value.to_string(), false)?;
        }

        let record = Record {
            header: RecordHeader {
                crc32: 0u32,
                key_size: key.len() as u64,
                value_size: value.len() as u64,
                tombstone: false,
            },
            key: key.to_string(),
            value: value.to_string(),
        };

        let (offset, timestamp) = {
            let mut active_file = self.active_file.lock().unwrap();
            let mut active_segment = self.active_segment.lock().unwrap();

            let seg_size = active_file.file.seek(SeekFrom::End(0))?;

            if self.max_segment_bytes.load(Relaxed) < seg_size + record.size_on_disk() {
                self.roll_segment(&mut active_file, &mut active_segment)?;
            }

            let offset = record.append(&mut active_file.file)?;
            (offset, active_segment.timestamp)
        };

        let mut index = self.index.write().unwrap();
        if let Some(old) = index.set(key.to_string(), offset, timestamp, record.size_on_disk()) {
            self.dead_bytes.fetch_add(old.record_size, Relaxed);
        }
        drop(index);
        self.total_bytes.fetch_add(record.size_on_disk(), Relaxed);
        self.fsync()?;

        Ok(())
    }

    /// Deletes a key from the database.
    ///
    /// Removes the key from the in-memory index and appends a tombstone
    /// record to the active segment. Returns `Ok(None)` if the key was not
    /// present.
    fn delete(&self, key: &str) -> Result<Option<()>, Error> {
        if key.len() > MAX_KEY_SIZE {
            return Err(Error::new(
                std::io::ErrorKind::InvalidInput,
                "key or value exceeds maximum allowed size",
            ));
        }

        {
            let mut wal = self.wal.lock().unwrap();
            wal.append(key.to_string(), String::new(), true)?;
        }

        let old = {
            let mut index = self.index.write().unwrap();
            index.delete(key)
        };
        match old {
            Some(entry) => {
                let record = Record {
                    header: RecordHeader {
                        crc32: 0u32,
                        key_size: key.len() as u64,
                        value_size: 0,
                        tombstone: true,
                    },
                    key: key.to_string(),
                    value: String::new(),
                };
                {
                    let mut active_file = self.active_file.lock().unwrap();
                    record.append(&mut active_file.file)?;
                }
                self.fsync()?;
                self.dead_bytes
                    .fetch_add(entry.record_size + record.size_on_disk(), Relaxed);
                self.total_bytes.fetch_add(record.size_on_disk(), Relaxed);

                Ok(Some(()))
            }
            None => Ok(None),
        }
    }

    fn dead_bytes(&self) -> u64 {
        self.dead_bytes.load(Relaxed)
    }

    fn total_bytes(&self) -> u64 {
        self.total_bytes.load(Relaxed)
    }

    fn segment_count(&self) -> usize {
        self.segment_count.load(Relaxed)
    }

    fn list_keys(&self) -> io::Result<Vec<String>> {
        Ok(self.index.read().unwrap().ls_keys().cloned().collect())
    }

    fn exists(&self, key: &str) -> bool {
        self.index.read().unwrap().contains(key)
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
