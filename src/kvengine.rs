use std::fs::{self, File, OpenOptions};
use std::io::{self, Error, Seek, SeekFrom};
use std::path::PathBuf;

use crate::engine::StorageEngine;
use crate::hash_index::HashIndex;
use crate::hint::{Hint, HintEntry};
use crate::record::{MAX_KEY_SIZE, MAX_VALUE_SIZE, Record, RecordHeader};
use crate::segment::{Segment, get_segments};
use crate::settings::FSyncStrategy;
use crate::worker::BackgroundWorker;

pub struct KVEngine {
    index: HashIndex,
    active_file: File,
    db_path: String,
    db_name: String,
    active_segment: Segment,
    max_segment_bytes: u64,
    writes_since_fsync: u64,
    fsync_strategy: FSyncStrategy,
    fsync_handle: Option<BackgroundWorker>,
    dead_bytes: u64,
    total_bytes: u64,
    segment_count: usize,
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
            index: HashIndex::new(),
            active_file: file,
            db_path: db_path.to_string(),
            db_name: db_name.to_string(),
            active_segment: segment,
            max_segment_bytes,
            writes_since_fsync: 0,
            fsync_strategy,
            fsync_handle,
            dead_bytes: 0,
            total_bytes: 0,
            segment_count: 1,
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
        let segment = active_segment.unwrap().to_owned();
        let fsync_handle = Self::spawn_fsync_worker(fsync_strategy, segment.path(db_dir));
        Ok(Some(KVEngine {
            index: hash_index,
            active_file: current_file.unwrap(),
            db_path: db_dir.to_string(),
            db_name: db_name.to_string(),
            active_segment: Segment {
                segment_name: segment.segment_name,
                timestamp: segment.timestamp,
            },
            max_segment_bytes,
            writes_since_fsync: 0,
            fsync_strategy,
            fsync_handle,
            dead_bytes: 0,
            total_bytes: size,
            segment_count: segments.len(),
        }))
    }

    /// Builds a compacted KVEngine from the current state, returning
    /// the new engine without modifying self.
    fn build_compacted(&self) -> Result<KVEngine, Error> {
        let old_segments = get_segments(&self.db_path, &self.db_name)?;
        let mut new_db = KVEngine::new(
            &self.db_path,
            &self.db_name,
            self.max_segment_bytes,
            self.fsync_strategy,
        )?;

        let keys: Vec<String> = self.index.ls_keys().cloned().collect();
        for k in keys {
            let value = match self.get(&k)? {
                Some((_, value)) => value,
                None => continue,
            };
            new_db.set(&k, &value)?;
        }

        new_db.active_file.sync_all()?;

        for segment in &old_segments {
            fs::remove_file(segment.path(&self.db_path))?;
            let _ = fs::remove_file(segment.hint_path(&self.db_path));
        }

        let new_segments = get_segments(&self.db_path, &self.db_name)?;
        for segment in &new_segments {
            let hint_entries: Vec<HintEntry> = new_db
                .index
                .ls_keys()
                .filter_map(|k| {
                    let entry = new_db.index.get(k).unwrap();
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

        Ok(new_db)
    }

    /// Closes the current active segment and opens a new one.
    fn roll_segment(&mut self) -> io::Result<()> {
        if let Some(worker) = self.fsync_handle.take() {
            drop(worker);
        }

        let segment = Segment::new(&self.db_name).map_err(io::Error::other)?;
        let file: File = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(segment.path(&self.db_path))?;
        self.active_file = file;
        self.active_segment = segment;
        self.fsync_handle =
            Self::spawn_fsync_worker(self.fsync_strategy, self.active_segment.path(&self.db_path));
        self.segment_count += 1;
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
    fn fsync(&mut self) -> io::Result<()> {
        self.writes_since_fsync += 1;
        match self.fsync_strategy {
            FSyncStrategy::Always => {
                self.writes_since_fsync = 0;
                self.active_file.sync_all()?
            }
            FSyncStrategy::Never => {}
            FSyncStrategy::EveryN(n) => {
                if n <= self.writes_since_fsync as usize {
                    self.writes_since_fsync = 0;
                    self.active_file.sync_all()?
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
    fn compact(&mut self) -> Result<(), Error> {
        let new_db = self.build_compacted()?;
        self.index = new_db.index;
        self.active_file = new_db.active_file;
        self.active_segment = new_db.active_segment;
        self.writes_since_fsync = new_db.writes_since_fsync;
        self.fsync_handle = new_db.fsync_handle;
        self.segment_count = 1;
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
        let entry = match self.index.get(key) {
            Some(o) => o,
            None => return Ok(None),
        };

        let mut file = if entry.segment_timestamp == self.active_segment.timestamp {
            File::open(self.active_segment.path(&self.db_path))?
        } else {
            let segment = Segment {
                segment_name: self.db_name.clone(),
                timestamp: entry.segment_timestamp,
            };
            File::open(segment.path(&self.db_path))?
        };

        file.seek(SeekFrom::Start(entry.offset))?;

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
    fn set(&mut self, key: &str, value: &str) -> Result<(), Error> {
        if key.len() > MAX_KEY_SIZE || value.len() > MAX_VALUE_SIZE {
            return Err(Error::new(
                std::io::ErrorKind::InvalidInput,
                "key or value exceeds maximum allowed size",
            ));
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

        let seg_size = self.active_file.seek(SeekFrom::End(0))?;

        if self.max_segment_bytes < seg_size + record.size_on_disk() {
            self.roll_segment()?;
        }
        let mut file = self.active_file.try_clone()?;
        let offset = record.append(&mut file)?;
        if let Some(old) = self.index.set(
            key.to_string(),
            offset,
            self.active_segment.timestamp,
            record.size_on_disk(),
        ) {
            self.dead_bytes += old.record_size;
        }
        self.total_bytes += record.size_on_disk();
        self.fsync()?;

        Ok(())
    }

    /// Deletes a key from the database.
    ///
    /// Removes the key from the in-memory index and appends a tombstone
    /// record to the active segment. Returns `Ok(None)` if the key was not
    /// present.
    fn delete(&mut self, key: &str) -> Result<Option<()>, Error> {
        if key.len() > MAX_KEY_SIZE {
            return Err(Error::new(
                std::io::ErrorKind::InvalidInput,
                "key or value exceeds maximum allowed size",
            ));
        }
        let mut file = self.active_file.try_clone()?;
        match self.index.delete(key) {
            Some(old) => {
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
                record.append(&mut file)?;
                self.fsync()?;
                self.dead_bytes += old.record_size + record.size_on_disk();
                self.total_bytes += record.size_on_disk();

                Ok(Some(()))
            }
            None => Ok(None),
        }
    }

    fn dead_bytes(&self) -> u64 {
        self.dead_bytes
    }

    fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    fn segment_count(&self) -> usize {
        self.segment_count
    }

    fn list_keys(&self) -> io::Result<Vec<String>> {
        Ok(self.index.ls_keys().cloned().collect())
    }

    fn exists(&self, key: &str) -> bool {
        self.index.contains(key)
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
