use std::fs::{self, File, OpenOptions};
use std::io::{self, Error, Seek, SeekFrom};

use crate::hash_index::HashIndex;
use crate::hint::{Hint, HintEntry};
use crate::record::{MAX_KEY_SIZE, MAX_VALUE_SIZE, Record, RecordHeader};
use crate::segment::{Segment, get_segments};
use crate::settings::FSyncStrategy;

pub struct DB {
    index: HashIndex,
    active_file: File,
    db_path: String,
    db_name: String,
    active_segment: Segment,
    max_segment_bytes: u64,
    writes_since_fsync: u64,
    fsync_strategy: FSyncStrategy,
}

impl DB {
    /// Creates a new, empty database in the given directory.
    ///
    /// Creates `db_path` if it does not exist, opens a fresh segment file,
    /// and returns a `DB` ready for reads and writes.
    pub fn new(
        db_path: &str,
        db_name: &str,
        max_segment_bytes: u64,
        fsync_strategy: FSyncStrategy,
    ) -> io::Result<DB> {
        std::fs::create_dir_all(db_path)?;
        let segment = Segment::new(db_name).map_err(io::Error::other)?;
        let file: File = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(segment.path(db_path))?;

        Ok(DB {
            index: HashIndex::new(),
            active_file: file,
            db_path: db_path.to_string(),
            db_name: db_name.to_string(),
            active_segment: segment,
            max_segment_bytes,
            writes_since_fsync: 0,
            fsync_strategy,
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
    ) -> Result<Option<DB>, Error> {
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

        for segment in segments.iter() {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(segment.path(db_dir))?;

            match Hint::read_file(segment.hint_path(db_dir)) {
                Ok(hints) => {
                    hints.iter().for_each(|entry| {
                        if !entry.tombstone {
                            hash_index.set(entry.key.clone(), entry.offset, segment.timestamp);
                        }
                    });
                }
                Err(_) => {
                    hash_index.merge_from_file(&mut file, segment.timestamp)?;
                }
            }
            current_file = Some(file);
            active_segment = Some(segment);
        }
        let segment = active_segment.unwrap().to_owned();
        Ok(Some(DB {
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
        }))
    }

    /// Retrieves the key-value pair associated with the given key.
    ///
    /// Looks up the key in the in-memory index, seeks to the record's byte
    /// offset in the appropriate segment file, and reads the full record
    /// (verifying its CRC32 checksum).
    ///
    /// Returns `Ok(None)` if the key is not in the index.
    pub fn get(&self, key: &str) -> Result<Option<(String, String)>, Error> {
        let entry = match self.index.get(key) {
            Some(o) => o,
            None => return Ok(None),
        };

        let mut file = if entry.segment_timestamp == self.active_segment.timestamp {
            self.active_file.try_clone()?
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
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), Error> {
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
        self.index
            .set(key.to_string(), offset, self.active_segment.timestamp);
        self.fsync()?;

        Ok(())
    }

    /// Deletes a key from the database.
    ///
    /// Removes the key from the in-memory index and appends a tombstone
    /// record to the active segment. Returns `Ok(None)` if the key was not
    /// present.
    pub fn delete(&mut self, key: &str) -> Result<Option<()>, Error> {
        if key.len() > MAX_KEY_SIZE {
            return Err(Error::new(
                std::io::ErrorKind::InvalidInput,
                "key or value exceeds maximum allowed size",
            ));
        }
        let mut file = self.active_file.try_clone()?;
        match self.index.delete(key) {
            Some(_) => {
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
                Ok(Some(()))
            }
            None => Ok(None),
        }
    }

    /// Compacts the database by rewriting only live key-value pairs into
    /// fresh segments, then deleting the old segment and hint files.
    ///
    /// Returns a new `DB` instance backed by the compacted segments.
    pub fn get_compacted(&self) -> Result<DB, Error> {
        let old_segments = get_segments(&self.db_path, &self.db_name)?;
        let mut new_db = DB::new(
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
        let segment = Segment::new(&self.db_name).map_err(io::Error::other)?;
        let file: File = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(segment.path(&self.db_path))?;
        self.active_file = file;
        self.active_segment = segment;
        Ok(())
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
        }

        Ok(())
    }
}
