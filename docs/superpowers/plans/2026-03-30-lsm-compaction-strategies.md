# LSM Compaction Strategies Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the flat `compact()` in `LsmEngine` with a pluggable `CompactionStrategy` trait supporting size-tiered and leveled (LevelDB-style) compaction, selectable via CLI.

**Architecture:** A `CompactionStrategy` trait encapsulates all SSTable organization and compaction logic. `LsmEngine` replaces `segments: Vec<SSTable>` with `strategy: Box<dyn CompactionStrategy>` and delegates reads, writes, and compaction to it. Two implementations: `SizeTieredCompaction` (default, groups files by size) and `LeveledCompaction` (L0–L3, no-overlap guarantee at L1+).

**Tech Stack:** Rust, Cargo. No new dependencies. All tests in `tests/`.

**Spec:** `docs/superpowers/specs/2026-03-30-lsm-compaction-strategies-design.md`

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `src/engine.rs` | Add `compact_step` default no-op to `StorageEngine` |
| Modify | `src/sstable.rs` | `rebuild_index` → `pub`; add `parse_leveled`, `get_sstables_leveled` |
| Create | `src/compaction.rs` | `CompactionStrategy` trait |
| Create | `src/size_tiered.rs` | `SizeTieredCompaction`, `SizeTieredConfig` |
| Create | `src/leveled.rs` | `LeveledCompaction`, `Level`, `LeveledConfig` |
| Modify | `src/lsmengine.rs` | Replace `segments` with `strategy: Box<dyn CompactionStrategy>` |
| Modify | `src/settings.rs` | Add `CompactionStrategyType`, leveled/size-tiered config fields |
| Modify | `src/main.rs` | Wire new settings; update background worker to call `compact_step` |
| Modify | `src/lib.rs` | Expose `compaction`, `size_tiered`, `leveled` modules |
| Create | `tests/size_tiered_compaction.rs` | Size-tiered unit + integration tests |
| Create | `tests/leveled_compaction.rs` | Leveled unit + integration tests |

---

## Task 1: Infrastructure — `compact_step` and `rebuild_index` visibility

**Files:**
- Modify: `src/engine.rs`
- Modify: `src/sstable.rs`

- [ ] **Step 1: Add `compact_step` to `StorageEngine`**

In `src/engine.rs`, add this method to the trait (after `compact`):

```rust
fn compact_step(&mut self) -> io::Result<bool> {
    Ok(false)
}
```

- [ ] **Step 2: Make `rebuild_index` pub**

In `src/sstable.rs` line 161, change:
```rust
fn rebuild_index(&mut self) -> io::Result<()> {
```
to:
```rust
/// Rebuild the sparse index and Bloom filter by scanning all records.
/// Used at startup and in tests.
pub fn rebuild_index(&mut self) -> io::Result<()> {
```

> Note: `pub` (not `pub(crate)`) because integration tests in `tests/` are a separate crate and cannot access `pub(crate)` items.

- [ ] **Step 3: Verify nothing broke**

```bash
cargo test
```
Expected: all existing tests pass.

- [ ] **Step 4: Commit**

```bash
cargo fmt
cargo clippy -- -D warnings
git add src/engine.rs src/sstable.rs
git commit -m "refactor: add compact_step to StorageEngine; pub rebuild_index"
```

---

## Task 2: `CompactionStrategy` trait

**Files:**
- Create: `src/compaction.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Create `src/compaction.rs`**

```rust
use std::io;
use crate::sstable::SSTable;

pub trait CompactionStrategy: Send + Sync {
    /// Called after every memtable flush.
    fn add_sstable(&mut self, sst: SSTable);

    /// Called by the background worker. Runs at most one compaction step.
    /// Returns true if a compaction step ran.
    fn compact_if_needed(&mut self, db_path: &str, db_name: &str) -> io::Result<bool>;

    /// Full compaction of all on-disk files. Called by the COMPACT command
    /// (after LsmEngine has already flushed the live memtable).
    fn compact_all(&mut self, db_path: &str, db_name: &str) -> io::Result<()>;

    /// Files to check for a key lookup, in priority order (newest first).
    fn iter_for_key<'a>(&'a self, key: &str) -> Box<dyn Iterator<Item = &'a SSTable> + 'a>;

    /// Files whose key range overlaps [start, end], for range scans.
    fn iter_files_for_range<'a>(
        &'a self,
        start: &str,
        end: &str,
    ) -> Box<dyn Iterator<Item = &'a SSTable> + 'a>;

    /// All SSTables, for list_keys.
    fn iter_all<'a>(&'a self) -> Box<dyn Iterator<Item = &'a SSTable> + 'a>;

    /// Total SSTable file count (LsmEngine delegates segment_count() here).
    fn segment_count(&self) -> usize;
}
```

- [ ] **Step 2: Add module to `src/lib.rs`**

```rust
pub mod compaction;
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo build
```
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
cargo fmt
cargo clippy -- -D warnings
git add src/compaction.rs src/lib.rs
git commit -m "feat: add CompactionStrategy trait"
```

---

## Task 3: `SizeTieredCompaction` — tests first

**Files:**
- Create: `tests/size_tiered_compaction.rs`
- Create: `src/size_tiered.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write the tests**

Create `tests/size_tiered_compaction.rs`:

```rust
use rustikv::compaction::CompactionStrategy;
use rustikv::memtable::Memtable;
use rustikv::size_tiered::{SizeTieredCompaction, SizeTieredConfig};
use rustikv::sstable::SSTable;
use std::{env, fs, time::SystemTime};

fn temp_dir(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = env::temp_dir();
    path.push(format!("kv_store_st_{}_{}", suffix, nanos));
    fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().to_string()
}

fn make_sstable(dir: &str, name: &str, entries: &[(&str, &str)]) -> SSTable {
    let mut memtable = Memtable::new();
    for (k, v) in entries {
        memtable.insert(k.to_string(), v.to_string());
    }
    SSTable::from_memtable(dir, name, &memtable).unwrap()
}

fn default_config() -> SizeTieredConfig {
    SizeTieredConfig {
        min_threshold: 4,
        max_threshold: 32,
        bucket_low: 0.5,
        bucket_high: 1.5,
    }
}

// Fewer than min_threshold files → no compaction
#[test]
fn below_threshold_does_not_compact() {
    let dir = temp_dir("below_threshold");
    let mut strategy = SizeTieredCompaction::new(default_config());
    for i in 0..3 {
        let sst = make_sstable(&dir, "test", &[(&format!("k{i}"), "v")]);
        strategy.add_sstable(sst);
    }
    let compacted = strategy.compact_if_needed(&dir, "test").unwrap();
    assert!(!compacted);
    assert_eq!(strategy.segment_count(), 3);
}

// Reaching min_threshold triggers compaction
#[test]
fn at_threshold_compacts() {
    let dir = temp_dir("at_threshold");
    let mut strategy = SizeTieredCompaction::new(default_config());
    for i in 0..4 {
        let sst = make_sstable(&dir, "test", &[(&format!("k{i}"), "v")]);
        strategy.add_sstable(sst);
    }
    let compacted = strategy.compact_if_needed(&dir, "test").unwrap();
    assert!(compacted);
    assert_eq!(strategy.segment_count(), 1);
}

// After compaction, reads return correct (newest) values
#[test]
fn reads_correct_after_compaction() {
    let dir = temp_dir("reads_after");
    let mut strategy = SizeTieredCompaction::new(default_config());
    // Write key "a" in first file, overwrite in second
    let sst1 = make_sstable(&dir, "test", &[("a", "old"), ("b", "one")]);
    let sst2 = make_sstable(&dir, "test", &[("a", "new"), ("c", "two")]);
    let sst3 = make_sstable(&dir, "test", &[("d", "three")]);
    let sst4 = make_sstable(&dir, "test", &[("e", "four")]);
    strategy.add_sstable(sst1);
    strategy.add_sstable(sst2);
    strategy.add_sstable(sst3);
    strategy.add_sstable(sst4);
    strategy.compact_if_needed(&dir, "test").unwrap();

    // Find "a" — should be "new" (newest wins)
    let val = strategy
        .iter_for_key("a")
        .find_map(|sst| sst.get("a").ok().flatten().flatten());
    assert_eq!(val.as_deref(), Some("new"));
}

// compact_all merges everything into one file
#[test]
fn compact_all_produces_single_file() {
    let dir = temp_dir("compact_all");
    let mut strategy = SizeTieredCompaction::new(default_config());
    for i in 0..6 {
        let sst = make_sstable(&dir, "test", &[(&format!("k{i}"), "v")]);
        strategy.add_sstable(sst);
    }
    strategy.compact_all(&dir, "test").unwrap();
    assert_eq!(strategy.segment_count(), 1);
}

// Stale keys are absent after compact_all
#[test]
fn stale_keys_removed_after_compact_all() {
    let dir = temp_dir("stale_keys");
    let mut strategy = SizeTieredCompaction::new(default_config());
    let sst1 = make_sstable(&dir, "test", &[("a", "old")]);
    let sst2 = make_sstable(&dir, "test", &[("a", "new")]);
    strategy.add_sstable(sst1);
    strategy.add_sstable(sst2);
    strategy.compact_all(&dir, "test").unwrap();
    // Only one file, and it has "new"
    let sst = strategy.iter_all().next().unwrap();
    let val = sst.get("a").unwrap().unwrap().unwrap();
    assert_eq!(val, "new");
}

// iter_files_for_range excludes files outside the requested range
#[test]
fn range_pruning_excludes_out_of_range_files() {
    let dir = temp_dir("range_pruning");
    let mut strategy = SizeTieredCompaction::new(default_config());
    // File 1: keys "a".."e" only
    let sst1 = make_sstable(&dir, "test", &[("a", "1"), ("b", "2"), ("c", "3")]);
    // File 2: keys "x".."z" only
    let sst2 = make_sstable(&dir, "test", &[("x", "4"), ("y", "5"), ("z", "6")]);
    strategy.add_sstable(sst1);
    strategy.add_sstable(sst2);

    // Querying ["a", "e"] should exclude the "x".."z" file
    let count = strategy.iter_files_for_range("a", "e").count();
    assert_eq!(count, 1, "expected 1 file for range [a,e], got {count}");
}

// Round-trip: compact_all then load_from_dir, reads still correct
#[test]
fn round_trip_compact_and_reload() {
    let dir = temp_dir("round_trip");
    let mut strategy = SizeTieredCompaction::new(default_config());
    let sst1 = make_sstable(&dir, "test", &[("x", "1"), ("y", "2")]);
    let sst2 = make_sstable(&dir, "test", &[("x", "updated")]);
    strategy.add_sstable(sst1);
    strategy.add_sstable(sst2);
    strategy.compact_all(&dir, "test").unwrap();

    let strategy2 = SizeTieredCompaction::load_from_dir(&dir, "test", default_config()).unwrap();
    let val = strategy2
        .iter_for_key("x")
        .find_map(|sst| sst.get("x").ok().flatten().flatten());
    assert_eq!(val.as_deref(), Some("updated"));
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cargo test --test size_tiered_compaction 2>&1 | head -20
```
Expected: compile error — `rustikv::size_tiered` not found.

- [ ] **Step 3: Create `src/size_tiered.rs`**

```rust
use std::{fs, io, path::PathBuf};

use crate::{
    compaction::CompactionStrategy,
    memtable::Memtable,
    record::Record,
    sstable::SSTable,
};

pub struct SizeTieredConfig {
    pub min_threshold: usize,
    pub max_threshold: usize,
    pub bucket_low: f64,
    pub bucket_high: f64,
}

impl Default for SizeTieredConfig {
    fn default() -> Self {
        SizeTieredConfig {
            min_threshold: 4,
            max_threshold: 32,
            bucket_low: 0.5,
            bucket_high: 1.5,
        }
    }
}

pub struct SizeTieredCompaction {
    files: Vec<SSTable>, // newest-to-oldest
    config: SizeTieredConfig,
}

impl SizeTieredCompaction {
    pub fn new(config: SizeTieredConfig) -> Self {
        SizeTieredCompaction {
            files: Vec::new(),
            config,
        }
    }

    pub fn load_from_dir(dir: &str, name: &str, config: SizeTieredConfig) -> io::Result<Self> {
        use crate::sstable::get_sstables;
        let files = get_sstables(dir, name)?;
        // get_sstables returns oldest-to-newest; we want newest-to-oldest
        let mut files = files;
        files.reverse();
        Ok(SizeTieredCompaction { files, config })
    }

    /// Returns file size in bytes (0 if the file cannot be stat'd).
    fn file_bytes(sst: &SSTable) -> u64 {
        sst.path.metadata().map(|m| m.len()).unwrap_or(0)
    }

    /// Groups files into buckets of similar size.
    /// A file belongs to a bucket if its size is within
    /// [bucket_avg * bucket_low, bucket_avg * bucket_high].
    fn form_buckets(&self) -> Vec<Vec<usize>> {
        // Work on indices into self.files, sorted by file size ascending
        let mut by_size: Vec<usize> = (0..self.files.len()).collect();
        by_size.sort_by_key(|&i| Self::file_bytes(&self.files[i]));

        let mut buckets: Vec<Vec<usize>> = Vec::new();
        for idx in by_size {
            let size = Self::file_bytes(&self.files[idx]) as f64;
            let placed = buckets.iter_mut().any(|bucket| {
                let avg = bucket
                    .iter()
                    .map(|&i| Self::file_bytes(&self.files[i]) as f64)
                    .sum::<f64>()
                    / bucket.len() as f64;
                if size >= avg * self.config.bucket_low && size <= avg * self.config.bucket_high {
                    bucket.push(idx);
                    true
                } else {
                    false
                }
            });
            if !placed {
                buckets.push(vec![idx]);
            }
        }
        buckets
    }

    /// Merges a set of files (by index) into a single new SSTable.
    /// Keeps the newest value per key; does not drop tombstones.
    fn merge_files(&self, indices: &[usize], db_path: &str, db_name: &str) -> io::Result<SSTable> {
        let mut memtable = Memtable::new();
        // Iterate oldest-to-newest so newer values overwrite older ones
        let mut sorted = indices.to_vec();
        sorted.sort_by_key(|&i| self.files[i].timestamp);
        for idx in &sorted {
            let sst = &self.files[*idx];
            for result in sst.iter()? {
                let record = result?;
                if record.header.tombstone {
                    memtable.remove(record.key);
                } else {
                    memtable.insert(record.key, record.value);
                }
            }
        }
        SSTable::from_memtable(db_path, db_name, &memtable)
    }
}

impl CompactionStrategy for SizeTieredCompaction {
    fn add_sstable(&mut self, sst: SSTable) {
        self.files.insert(0, sst); // newest first
    }

    fn compact_if_needed(&mut self, db_path: &str, db_name: &str) -> io::Result<bool> {
        let buckets = self.form_buckets();
        let ready = buckets
            .into_iter()
            .find(|b| b.len() >= self.config.min_threshold);

        let Some(indices) = ready else {
            return Ok(false);
        };

        // Delete old files BEFORE writing the new one to avoid nanosecond
        // timestamp collisions with create_new(true) in from_memtable.
        for &idx in &indices {
            fs::remove_file(&self.files[idx].path)?;
        }

        let new_sst = self.merge_files(&indices, db_path, db_name)?;

        // Remove from in-memory list (reverse order to keep indices stable)
        let mut sorted_desc = indices.clone();
        sorted_desc.sort_unstable_by(|a, b| b.cmp(a));
        for idx in sorted_desc {
            self.files.remove(idx);
        }

        self.files.insert(0, new_sst);
        Ok(true)
    }

    fn compact_all(&mut self, db_path: &str, db_name: &str) -> io::Result<()> {
        if self.files.is_empty() {
            return Ok(());
        }
        // Delete old files BEFORE calling merge_files so that from_memtable's
        // create_new(true) does not collide with existing files sharing the same
        // nanosecond timestamp.
        for sst in &self.files {
            fs::remove_file(&sst.path)?;
        }
        let all_indices: Vec<usize> = (0..self.files.len()).collect();
        let new_sst = self.merge_files(&all_indices, db_path, db_name)?;
        self.files = vec![new_sst];
        Ok(())
    }

    fn iter_for_key<'a>(&'a self, _key: &str) -> Box<dyn Iterator<Item = &'a SSTable> + 'a> {
        // Size-tiered has no overlap guarantee — all files must be checked.
        // The key parameter is intentionally unused.
        Box::new(self.files.iter())
    }

    fn iter_files_for_range<'a>(
        &'a self,
        start: &str,
        end: &str,
    ) -> Box<dyn Iterator<Item = &'a SSTable> + 'a> {
        let start = start.to_string();
        let end = end.to_string();
        Box::new(self.files.iter().filter(move |sst| {
            let min = sst.get_min().as_ref().map(|(k, _)| k.as_str()).unwrap_or("");
            let max = sst.get_max().as_ref().map(|(k, _)| k.as_str()).unwrap_or("");
            max >= start.as_str() && min <= end.as_str()
        }))
    }

    fn iter_all<'a>(&'a self) -> Box<dyn Iterator<Item = &'a SSTable> + 'a> {
        Box::new(self.files.iter())
    }

    fn segment_count(&self) -> usize {
        self.files.len()
    }
}
```

- [ ] **Step 4: Add module to `src/lib.rs`**

```rust
pub mod size_tiered;
```

- [ ] **Step 5: Run tests**

```bash
cargo test --test size_tiered_compaction
```
Expected: all 6 tests pass.

- [ ] **Step 6: Run full suite**

```bash
cargo test
```
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
cargo fmt
cargo clippy -- -D warnings
git add src/compaction.rs src/size_tiered.rs src/lib.rs tests/size_tiered_compaction.rs
git commit -m "feat: add SizeTieredCompaction strategy with tests"
```

---

## Task 4: Wire `SizeTieredCompaction` into `LsmEngine`

**Files:**
- Modify: `src/lsmengine.rs`

- [ ] **Step 1: Replace `segments` with `strategy` in `LsmEngine`**

Replace the struct definition and all methods. New `lsmengine.rs`:

```rust
use std::ops::Bound::Included;
use std::{any::Any, collections::HashSet, io, path::PathBuf};

use crate::{
    compaction::CompactionStrategy,
    engine::{RangeScan, StorageEngine},
    memtable::Memtable,
    size_tiered::{SizeTieredCompaction, SizeTieredConfig},
    sstable::SSTable,
    wal::Wal,
};

pub struct LsmEngine {
    memtable: Memtable,
    db_path: String,
    db_name: String,
    max_memtable_bytes: usize,
    wal: Wal,
    strategy: Box<dyn CompactionStrategy>,
}

impl LsmEngine {
    pub fn new(db_path: &str, db_name: &str, max_memtable_bytes: usize) -> io::Result<LsmEngine> {
        let wal = Wal::open(&PathBuf::from(db_path), db_name.to_string())?;
        Ok(LsmEngine {
            memtable: Memtable::new(),
            db_path: db_path.to_string(),
            db_name: db_name.to_string(),
            max_memtable_bytes,
            wal,
            strategy: Box::new(SizeTieredCompaction::new(SizeTieredConfig::default())),
        })
    }

    pub fn from_dir(dir: &str, db_name: &str, max_memtable_bytes: usize) -> io::Result<Self> {
        let wal = Wal::open(&PathBuf::from(dir), db_name.to_string())?;
        let memtable = wal.replay()?;
        let strategy = SizeTieredCompaction::load_from_dir(dir, db_name, SizeTieredConfig::default())?;
        Ok(LsmEngine {
            memtable,
            db_path: dir.to_string(),
            db_name: db_name.to_string(),
            max_memtable_bytes,
            wal,
            strategy: Box::new(strategy),
        })
    }

    fn flush_memtable(&mut self) -> io::Result<()> {
        let sst = SSTable::from_memtable(&self.db_path, &self.db_name, &self.memtable)?;
        self.strategy.add_sstable(sst);
        self.memtable.clear();
        self.wal.reset()?;
        Ok(())
    }
}

impl StorageEngine for LsmEngine {
    fn get(&self, key: &str) -> Result<Option<(String, String)>, io::Error> {
        match self.memtable.entry(key) {
            Some(Some(v)) => return Ok(Some((key.to_string(), v.clone()))),
            Some(None) => return Ok(None),
            None => {}
        }
        for segment in self.strategy.iter_for_key(key) {
            match segment.get(key)? {
                Some(Some(v)) => return Ok(Some((key.to_string(), v))),
                Some(None) => return Ok(None),
                None => continue,
            }
        }
        Ok(None)
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), io::Error> {
        self.wal.append(key.to_string(), value.to_string(), false)?;
        self.memtable.insert(key.to_string(), value.to_string());
        if self.memtable.size_bytes() >= self.max_memtable_bytes {
            self.flush_memtable()?;
        }
        Ok(())
    }

    fn delete(&mut self, key: &str) -> Result<Option<()>, io::Error> {
        let exists = self.get(key)?.is_some();
        self.wal.append(key.to_string(), String::new(), true)?;
        self.memtable.remove(key.to_string());
        if self.memtable.size_bytes() >= self.max_memtable_bytes {
            self.flush_memtable()?;
        }
        if exists { Ok(Some(())) } else { Ok(None) }
    }

    fn compact(&mut self) -> Result<(), io::Error> {
        if !self.memtable.entries().is_empty() {
            self.flush_memtable()?;
        }
        self.strategy.compact_all(&self.db_path, &self.db_name)
    }

    fn compact_step(&mut self) -> io::Result<bool> {
        self.strategy.compact_if_needed(&self.db_path, &self.db_name)
    }

    fn dead_bytes(&self) -> u64 {
        0
    }

    fn total_bytes(&self) -> u64 {
        0
    }

    fn segment_count(&self) -> usize {
        self.strategy.segment_count()
    }

    fn list_keys(&self) -> io::Result<Vec<String>> {
        let mut keys: HashSet<String> = HashSet::new();
        for segment in self.strategy.iter_all() {
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

    fn mget(&self, keys: Vec<String>) -> Result<Vec<(String, Option<String>)>, io::Error> {
        keys.into_iter()
            .map(|key| {
                let val = self.get(&key)?.map(|(_, v)| v);
                Ok((key, val))
            })
            .collect()
    }

    fn mset(&mut self, items: Vec<(String, String)>) -> Result<(), io::Error> {
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
        use std::collections::BTreeMap;
        if start > end {
            return Ok(vec![]);
        }
        let mut b_map: BTreeMap<String, String> = BTreeMap::new();
        for segment in self.strategy.iter_files_for_range(start, end) {
            for result in segment.iter()? {
                let record = result?;
                if record.key.as_str() < start || record.key.as_str() > end {
                    continue;
                }
                if record.header.tombstone {
                    b_map.remove(&record.key);
                } else {
                    b_map.insert(record.key, record.value);
                }
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
```

- [ ] **Step 2: Run all tests**

```bash
cargo test
```
Expected: all tests pass. If `compact_step` causes a compile error (`method not found in StorageEngine`), check that `src/engine.rs` has the default implementation from Task 1.

- [ ] **Step 3: Commit**

```bash
cargo fmt
cargo clippy -- -D warnings
git add src/lsmengine.rs
git commit -m "refactor: wire SizeTieredCompaction into LsmEngine"
```

---

## Task 5: CLI flags for compaction strategy

**Files:**
- Modify: `src/settings.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Add types to `src/settings.rs`**

Add after the `EngineType` enum:

```rust
#[derive(Copy, Clone)]
pub enum CompactionStrategyType {
    SizeTiered,
    Leveled,
}

pub struct SizeTieredSettings {
    pub min_threshold: usize,
    pub max_threshold: usize,
}

pub struct LeveledSettings {
    pub l0_compaction_threshold: usize,
    pub l1_max_bytes: u64,
    pub level_multiplier: u64,
    pub max_levels: usize,
    pub max_file_bytes: u64,
}
```

Add fields to `Settings`:

```rust
pub compaction_strategy: CompactionStrategyType,
pub size_tiered: SizeTieredSettings,
pub leveled: LeveledSettings,
```

Set defaults in `Settings::get_from_args()`:

```rust
compaction_strategy: CompactionStrategyType::SizeTiered,
size_tiered: SizeTieredSettings { min_threshold: 4, max_threshold: 32 },
leveled: LeveledSettings {
    l0_compaction_threshold: 4,
    l1_max_bytes: 10_485_760,
    level_multiplier: 10,
    max_levels: 4,
    max_file_bytes: 2_097_152,
},
```

Add CLI parsing in the `while let Some(arg)` loop:

```rust
"-cs" | "--compaction-strategy" => {
    if let Some(value) = args_iter.next() {
        settings.compaction_strategy = match value.as_str() {
            "leveled" => CompactionStrategyType::Leveled,
            "size-tiered" => CompactionStrategyType::SizeTiered,
            _ => panic!("Unknown compaction strategy: {value}"),
        };
    }
}
"-l0t" | "--l0-compaction-threshold" => {
    if let Some(value) = args_iter.next() {
        settings.leveled.l0_compaction_threshold =
            value.parse().expect("Invalid l0 threshold");
    }
}
"-l1b" | "--l1-max-bytes" => {
    if let Some(value) = args_iter.next() {
        settings.leveled.l1_max_bytes = value.parse().expect("Invalid l1 max bytes");
    }
}
"-lm" | "--level-multiplier" => {
    if let Some(value) = args_iter.next() {
        settings.leveled.level_multiplier =
            value.parse().expect("Invalid level multiplier");
    }
}
"-ml" | "--max-levels" => {
    if let Some(value) = args_iter.next() {
        settings.leveled.max_levels = value.parse().expect("Invalid max levels");
    }
}
"-mfb" | "--max-file-bytes" => {
    if let Some(value) = args_iter.next() {
        settings.leveled.max_file_bytes = value.parse().expect("Invalid max file bytes");
    }
}
"-stmin" | "--st-min-threshold" => {
    if let Some(value) = args_iter.next() {
        settings.size_tiered.min_threshold = value.parse().expect("Invalid st min");
    }
}
"-stmax" | "--st-max-threshold" => {
    if let Some(value) = args_iter.next() {
        settings.size_tiered.max_threshold = value.parse().expect("Invalid st max");
    }
}
```

Update `print_help` to document the new flags.

- [ ] **Step 2: Wire settings in `src/main.rs`**

Find where `LsmEngine::from_dir` / `LsmEngine::new` is called and update it to pass the compaction strategy. For now, since leveled is not implemented yet, just pass `SizeTieredConfig` from settings. Add a `use rustikv::size_tiered::SizeTieredConfig;` import and pass:

```rust
// Replace LsmEngine::new(path, name, max_mem) with:
LsmEngine::new_with_config(
    path,
    name,
    max_mem,
    SizeTieredConfig {
        min_threshold: settings.size_tiered.min_threshold,
        max_threshold: settings.size_tiered.max_threshold,
        ..SizeTieredConfig::default()
    },
)
```

Add two constructors to `src/lsmengine.rs`:

```rust
pub fn new_with_config(
    db_path: &str,
    db_name: &str,
    max_memtable_bytes: usize,
    st_config: SizeTieredConfig,
) -> io::Result<LsmEngine> {
    let wal = Wal::open(&PathBuf::from(db_path), db_name.to_string())?;
    Ok(LsmEngine {
        memtable: Memtable::new(),
        db_path: db_path.to_string(),
        db_name: db_name.to_string(),
        max_memtable_bytes,
        wal,
        strategy: Box::new(SizeTieredCompaction::new(st_config)),
    })
}

pub fn from_dir_with_config(
    dir: &str,
    db_name: &str,
    max_memtable_bytes: usize,
    st_config: SizeTieredConfig,
) -> io::Result<Self> {
    let wal = Wal::open(&PathBuf::from(dir), db_name.to_string())?;
    let memtable = wal.replay()?;
    let strategy = SizeTieredCompaction::load_from_dir(dir, db_name, st_config)?;
    Ok(LsmEngine {
        memtable,
        db_path: dir.to_string(),
        db_name: db_name.to_string(),
        max_memtable_bytes,
        wal,
        strategy: Box::new(strategy),
    })
}
```

- [ ] **Step 3: Run all tests**

```bash
cargo test
```
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/settings.rs src/main.rs src/lsmengine.rs
git commit -m "feat: add compaction strategy CLI flags"
```

---

## Task 6: `SSTable::parse_leveled` and `get_sstables_leveled`

**Files:**
- Modify: `src/sstable.rs`

- [ ] **Step 1: Add `parse_leveled` to `src/sstable.rs`**

Add after the existing `parse` function:

```rust
/// Parse a leveled SSTable filename: `{name}_L{level}_{timestamp}.sst`
/// or `{name}_L{level}_{timestamp}_{seq}.sst` (multi-file output).
/// Returns `(stub SSTable, level)`. Caller must call `rebuild_index` before use.
pub fn parse_leveled(filename: &str) -> Option<(Self, usize)> {
    let stem = filename.strip_suffix(".sst")?;
    // Split on `_L` to get name and the rest
    let (name, rest) = stem.split_once("_L")?;
    // rest is `{level}_{timestamp}` or `{level}_{timestamp}_{seq}`
    let (level_str, ts_and_seq) = rest.split_once('_')?;
    let level: usize = level_str.parse().ok()?;
    // timestamp is the first numeric segment before any optional `_{seq}`
    let timestamp_str = ts_and_seq.split('_').next()?;
    let timestamp: u64 = timestamp_str.parse().ok()?;

    Some((
        SSTable {
            path: PathBuf::new(),
            timestamp,
            name: name.to_string(),
            sparse_index: Vec::new(),
            bloom: BloomFilter::new(1, BLOOM_HASH_COUNT),
            min: None,
            max: None,
        },
        level,
    ))
}
```

- [ ] **Step 2: Add `get_sstables_leveled`**

Add after `get_sstables`:

```rust
/// Load leveled SSTables from a directory, grouped by level.
/// Returns a Vec of Vec<SSTable> indexed by level number (index 0 = L0).
/// Files not matching the leveled format are ignored.
pub fn get_sstables_leveled(
    dir: &str,
    db_name: &str,
    max_levels: usize,
) -> io::Result<Vec<Vec<SSTable>>> {
    let mut levels: Vec<Vec<SSTable>> = (0..max_levels).map(|_| Vec::new()).collect();

    for entry in read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let Some((mut table, level)) = SSTable::parse_leveled(&name) else {
            continue;
        };
        if table.name != db_name || level >= max_levels {
            continue;
        }
        table.path = PathBuf::from(dir).join(&name);
        table.rebuild_index()?;
        levels[level].push(table);
    }

    // L0: newest-to-oldest by timestamp. L1+: sorted by min_key.
    levels[0].sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    for level in levels.iter_mut().skip(1) {
        level.sort_by(|a, b| {
            let ak = a.get_min().as_ref().map(|(k, _)| k.as_str()).unwrap_or("");
            let bk = b.get_min().as_ref().map(|(k, _)| k.as_str()).unwrap_or("");
            ak.cmp(bk)
        });
    }

    Ok(levels)
}
```

- [ ] **Step 3: Verify compilation and tests**

```bash
cargo test
```
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/sstable.rs
git commit -m "feat: add SSTable::parse_leveled and get_sstables_leveled"
```

---

## Task 7: `LeveledCompaction` — tests first

**Files:**
- Create: `tests/leveled_compaction.rs`
- Create: `src/leveled.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write the tests**

Create `tests/leveled_compaction.rs`:

```rust
use rustikv::compaction::CompactionStrategy;
use rustikv::leveled::{LeveledCompaction, LeveledConfig};
use rustikv::memtable::Memtable;
use rustikv::sstable::SSTable;
use std::{env, fs, time::SystemTime};

fn temp_dir(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = env::temp_dir();
    path.push(format!("kv_store_lv_{}_{}", suffix, nanos));
    fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().to_string()
}

fn small_config() -> LeveledConfig {
    LeveledConfig {
        l0_compaction_threshold: 2, // trigger quickly in tests
        l1_max_bytes: 1024,         // 1 KB — easy to overflow
        level_multiplier: 4,
        max_levels: 3,
        max_file_bytes: 512,
    }
}

fn write_sst_to_strategy(strategy: &mut LeveledCompaction, dir: &str, entries: &[(&str, &str)]) {
    // Sleep 1ms between calls to guarantee distinct nanosecond timestamps.
    // SSTable::from_memtable uses create_new(true), so two files created in the
    // same nanosecond would collide. Tests that call this multiple times must
    // tolerate the 1ms delay.
    std::thread::sleep(std::time::Duration::from_millis(1));

    let mut memtable = Memtable::new();
    for (k, v) in entries {
        memtable.insert(k.to_string(), v.to_string());
    }
    let sst = SSTable::from_memtable(dir, "test", &memtable).unwrap();
    // Rename to L0 format before adding
    let l0_name = format!("test_L0_{}.sst", sst.timestamp);
    let new_path = std::path::PathBuf::from(dir).join(&l0_name);
    fs::rename(&sst.path, &new_path).unwrap();
    // Reload with correct path and rebuilt index
    let (mut stub, _level) = SSTable::parse_leveled(&l0_name).unwrap();
    stub.path = new_path;
    stub.rebuild_index().unwrap();
    strategy.add_sstable(stub);
}

// L0 compaction does not trigger below threshold
#[test]
fn l0_below_threshold_no_compaction() {
    let dir = temp_dir("l0_below");
    let mut strategy = LeveledCompaction::new(small_config());
    write_sst_to_strategy(&mut strategy, &dir, &[("a", "1")]);
    let ran = strategy.compact_if_needed(&dir, "test").unwrap();
    assert!(!ran);
}

// L0 triggers when reaching threshold
#[test]
fn l0_threshold_triggers_compaction() {
    let dir = temp_dir("l0_trigger");
    let mut strategy = LeveledCompaction::new(small_config());
    write_sst_to_strategy(&mut strategy, &dir, &[("a", "1"), ("b", "2")]);
    write_sst_to_strategy(&mut strategy, &dir, &[("c", "3"), ("d", "4")]);
    let ran = strategy.compact_if_needed(&dir, "test").unwrap();
    assert!(ran);
    // L0 should be empty, L1 should have files
    assert_eq!(strategy.l0_file_count(), 0);
    assert!(strategy.l1_file_count() > 0);
}

// After L0→L1 compaction, L1 files have non-overlapping key ranges
#[test]
fn l1_files_no_overlap_after_l0_compaction() {
    let dir = temp_dir("no_overlap");
    let mut strategy = LeveledCompaction::new(small_config());
    write_sst_to_strategy(&mut strategy, &dir, &[("a", "1"), ("b", "2")]);
    write_sst_to_strategy(&mut strategy, &dir, &[("c", "3"), ("d", "4")]);
    strategy.compact_if_needed(&dir, "test").unwrap();

    // Check no two L1 files overlap
    let ranges: Vec<(String, String)> = strategy.l1_key_ranges();

    for i in 0..ranges.len() {
        for j in (i + 1)..ranges.len() {
            let (min_i, max_i) = &ranges[i];
            let (min_j, max_j) = &ranges[j];
            assert!(
                max_i < min_j || max_j < min_i,
                "L1 files {i} and {j} overlap: [{min_i},{max_i}] vs [{min_j},{max_j}]"
            );
        }
    }
}

// Tombstone is not dropped when compacting into a non-final level
#[test]
fn tombstone_not_dropped_at_non_final_level() {
    let dir = temp_dir("tombstone");
    let mut strategy = LeveledCompaction::new(small_config());
    // Write "a" to L0 first (will end up in L1 eventually)
    write_sst_to_strategy(&mut strategy, &dir, &[("a", "original")]);
    write_sst_to_strategy(&mut strategy, &dir, &[("b", "other")]);
    strategy.compact_if_needed(&dir, "test").unwrap(); // L0→L1

    // Now write a tombstone for "a" back to L0
    let mut memtable = Memtable::new();
    memtable.remove("a".to_string());
    let sst = SSTable::from_memtable(&dir, "test", &memtable).unwrap();
    let l0_name = format!("test_L0_{}.sst", sst.timestamp);
    let new_path = std::path::PathBuf::from(&dir).join(&l0_name);
    fs::rename(&sst.path, &new_path).unwrap();
    let (mut stub, _) = SSTable::parse_leveled(&l0_name).unwrap();
    stub.path = new_path;
    stub.rebuild_index().unwrap();
    strategy.add_sstable(stub);

    write_sst_to_strategy(&mut strategy, &dir, &[("z", "filler")]);
    strategy.compact_if_needed(&dir, "test").unwrap(); // L0→L1 again

    // "a" must NOT be found (tombstone must have suppressed it)
    let found = strategy
        .iter_for_key("a")
        .find_map(|sst| sst.get("a").ok().flatten());
    // found == Some(None) means tombstone; found == None means absent
    assert!(
        found != Some(Some("original".to_string())),
        "stale value 'original' visible after tombstone"
    );
}

// get returns the newest value across levels
#[test]
fn reads_newest_value_across_levels() {
    let dir = temp_dir("newest_value");
    let mut strategy = LeveledCompaction::new(small_config());
    write_sst_to_strategy(&mut strategy, &dir, &[("k", "old")]);
    write_sst_to_strategy(&mut strategy, &dir, &[("k", "new"), ("x", "1")]);
    strategy.compact_if_needed(&dir, "test").unwrap();

    let val = strategy
        .iter_for_key("k")
        .find_map(|sst| sst.get("k").ok().flatten().flatten());
    assert_eq!(val.as_deref(), Some("new"));
}

// compact_all leaves all levels within budget
#[test]
fn compact_all_leaves_no_level_needing_compaction() {
    let dir = temp_dir("compact_all");
    let mut strategy = LeveledCompaction::new(small_config());
    for i in 0..6 {
        write_sst_to_strategy(&mut strategy, &dir, &[(&format!("k{i}"), "v")]);
    }
    strategy.compact_all(&dir, "test").unwrap();
    let ran = strategy.compact_if_needed(&dir, "test").unwrap();
    assert!(!ran, "compact_if_needed should return false after compact_all");
}

// Restart: reload from dir produces correct reads
#[test]
fn restart_and_reload() {
    let dir = temp_dir("restart");
    {
        let mut strategy = LeveledCompaction::new(small_config());
        write_sst_to_strategy(&mut strategy, &dir, &[("a", "1"), ("b", "2")]);
        write_sst_to_strategy(&mut strategy, &dir, &[("c", "3"), ("d", "4")]);
        strategy.compact_if_needed(&dir, "test").unwrap();
    }
    // Reload
    let strategy2 = LeveledCompaction::load_from_dir(&dir, "test", small_config()).unwrap();
    let val = strategy2
        .iter_for_key("b")
        .find_map(|sst| sst.get("b").ok().flatten().flatten());
    assert_eq!(val.as_deref(), Some("2"));
}
```

- [ ] **Step 2: Run tests to confirm compile error**

```bash
cargo test --test leveled_compaction 2>&1 | head -20
```
Expected: compile error — `rustikv::leveled` not found.

- [ ] **Step 3: Create `src/leveled.rs`**

The implementation must expose the public methods referenced in the tests:
- `LeveledCompaction::new(config)`
- `LeveledCompaction::load_from_dir(dir, name, config)`
- `l0_file_count() -> usize` (test helper, pub)
- `l1_file_count() -> usize` (test helper, pub)
- `l1_key_ranges() -> Vec<(String, String)>` (test helper, pub — use owned Strings to avoid borrow issues when consuming the Vec)
- Implements `CompactionStrategy`

Key points for the implementation:
- `levels: Vec<Level>` — always `max_levels` entries
- Write output files with the `{name}_L{level}_{timestamp}.sst` naming convention
- For multi-file output, append a sequence counter: `{name}_L{level}_{timestamp}_{seq}.sst`
- Tombstones are preserved unless the target level is the final level (`level_num == max_levels - 1`)
- `compact_if_needed` scans from L0 outward and runs at most one compaction step
- `compact_all` loops over `compact_if_needed` until it returns `false`
- `iter_for_key`: L0 files newest-to-oldest, then for L1+ at most one file per level (binary search on key range)

**`Level` struct:**
```rust
pub struct Level {
    pub level_num: usize,
    pub max_bytes: u64,
    pub files: Vec<SSTable>,
}

impl Level {
    pub fn total_bytes(&self) -> u64 { ... }
    pub fn needs_compaction(&self, l0_threshold: usize) -> bool { ... }
    pub fn overlapping_files(&self, min_key: &str, max_key: &str) -> Vec<usize> {
        // Returns indices of files whose [min, max] overlaps [min_key, max_key]
    }
    fn key_range_of_file(sst: &SSTable) -> Option<(String, String)> { ... }
}
```

**`LeveledCompaction::merge_into_level`** (core helper):
```rust
fn merge_into_level(
    &mut self,
    source_indices: Vec<(usize, usize)>, // (level_num, file_idx)
    target_level: usize,
    db_path: &str,
    db_name: &str,
) -> io::Result<()>
```

- [ ] **Step 4: Add module to `src/lib.rs`**

```rust
pub mod leveled;
```

- [ ] **Step 5: Run tests**

```bash
cargo test --test leveled_compaction
```
Expected: all 7 tests pass.

- [ ] **Step 6: Run full suite**

```bash
cargo clippy -- -D warnings
cargo test
```
Expected: zero warnings, all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/leveled.rs src/lib.rs tests/leveled_compaction.rs
git commit -m "feat: add LeveledCompaction strategy with tests"
```

---

## Task 8: Wire `LeveledCompaction` into `LsmEngine` and CLI

**Files:**
- Modify: `src/lsmengine.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Add `LsmEngine::new_leveled` and `LsmEngine::from_dir_leveled` constructors**

In `src/lsmengine.rs`:

```rust
use crate::leveled::{LeveledCompaction, LeveledConfig};

pub fn new_leveled(
    db_path: &str,
    db_name: &str,
    max_memtable_bytes: usize,
    config: LeveledConfig,
) -> io::Result<LsmEngine> {
    let wal = Wal::open(&PathBuf::from(db_path), db_name.to_string())?;
    Ok(LsmEngine {
        memtable: Memtable::new(),
        db_path: db_path.to_string(),
        db_name: db_name.to_string(),
        max_memtable_bytes,
        wal,
        strategy: Box::new(LeveledCompaction::new(config)),
    })
}

pub fn from_dir_leveled(
    dir: &str,
    db_name: &str,
    max_memtable_bytes: usize,
    config: LeveledConfig,
) -> io::Result<Self> {
    let wal = Wal::open(&PathBuf::from(dir), db_name.to_string())?;
    let memtable = wal.replay()?;
    let strategy = LeveledCompaction::load_from_dir(dir, db_name, config)?;
    Ok(LsmEngine {
        memtable,
        db_path: dir.to_string(),
        db_name: db_name.to_string(),
        max_memtable_bytes,
        wal,
        strategy: Box::new(strategy),
    })
}
```

- [ ] **Step 2: Wire leveled into `src/main.rs`**

Find where `LsmEngine::new` / `LsmEngine::from_dir` is called. Add a branch for `CompactionStrategyType::Leveled`:

Look at the existing `main.rs` to find the exact location where `LsmEngine` is constructed (search for `LsmEngine::from_dir` or `LsmEngine::new`). The current code likely uses `from_dir` unconditionally. Replace it with a strategy-aware branch:

```rust
use rustikv::leveled::LeveledConfig;
use rustikv::settings::CompactionStrategyType;

// Check if the db directory already contains SSTable files.
// `from_dir`-style constructors handle both empty and existing dirs,
// but we use separate constructors to pass the right config.
let dir_has_data = std::fs::read_dir(&settings.db_file_path)
    .map(|mut d| d.next().is_some())
    .unwrap_or(false);

let engine: Box<dyn StorageEngine> = match settings.compaction_strategy {
    CompactionStrategyType::SizeTiered => {
        let st_cfg = SizeTieredConfig {
            min_threshold: settings.size_tiered.min_threshold,
            max_threshold: settings.size_tiered.max_threshold,
            ..SizeTieredConfig::default()
        };
        if dir_has_data {
            Box::new(LsmEngine::from_dir_with_config(&settings.db_file_path, &settings.db_name, max_mem, st_cfg)?)
        } else {
            Box::new(LsmEngine::new_with_config(&settings.db_file_path, &settings.db_name, max_mem, st_cfg)?)
        }
    }
    CompactionStrategyType::Leveled => {
        let lv_cfg = LeveledConfig {
            l0_compaction_threshold: settings.leveled.l0_compaction_threshold,
            l1_max_bytes: settings.leveled.l1_max_bytes,
            level_multiplier: settings.leveled.level_multiplier,
            max_levels: settings.leveled.max_levels,
            max_file_bytes: settings.leveled.max_file_bytes,
        };
        if dir_has_data {
            Box::new(LsmEngine::from_dir_leveled(&settings.db_file_path, &settings.db_name, max_mem, lv_cfg)?)
        } else {
            Box::new(LsmEngine::new_leveled(&settings.db_file_path, &settings.db_name, max_mem, lv_cfg)?)
        }
    }
};
```

- [ ] **Step 3: Run all tests**

```bash
cargo test
```
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/lsmengine.rs src/main.rs
git commit -m "feat: wire LeveledCompaction into LsmEngine and CLI"
```

---

## Task 9: Background worker — call `compact_step`

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Update `maybe_trigger_compaction`**

Currently the function only calls `db.compact()` (full compaction) when thresholds are crossed. Add a second path that calls `compact_step()` for the leveled/size-tiered incremental compaction. The worker should call `compact_step` on every tick regardless of the ratio/segment-count thresholds (those thresholds remain for `compact()` — the full compaction):

```rust
fn maybe_trigger_compaction(
    database: Arc<RwLock<Box<dyn StorageEngine>>>,
    stats: &Arc<Stats>,
    compaction_ratio: f32,
    compaction_max_segment: usize,
) {
    let db_clone_read = Arc::clone(&database);
    let db_clone_write = Arc::clone(&database);

    let should_full_compact = {
        let db = db_clone_read.read().unwrap();
        (compaction_ratio > 0.0
            && db.total_bytes() > 0
            && db.dead_bytes() as f32 / db.total_bytes() as f32 > compaction_ratio)
            || (compaction_max_segment > 0 && db.segment_count() > compaction_max_segment)
    };

    // Always attempt an incremental compaction step (compact_step is a no-op
    // for KVEngine and for strategies with nothing to do).
    if stats
        .compacting
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    let stats_clone = Arc::clone(stats);
    thread::spawn(move || {
        let mut db = db_clone_write.write().unwrap();
        if should_full_compact {
            db.compact().unwrap();
        } else {
            db.compact_step().unwrap();
        }
        stats_clone
            .last_compact_end_ms
            .store(Stats::now_ms(), Ordering::Relaxed);
        stats_clone.compacting.store(false, Ordering::Release);
        stats_clone.compaction_count.fetch_add(1, Ordering::Relaxed);
    });
}
```

- [ ] **Step 2: Run full checklist**

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```
Expected: formatted, zero warnings, all tests pass.

- [ ] **Step 3: Final commit**

```bash
git add src/main.rs
git commit -m "feat: background worker calls compact_step for incremental compaction"
```

---

## Done

At this point:
- `SizeTieredCompaction` is the default and replaces the old flat `compact()`
- `LeveledCompaction` is available via `--compaction-strategy leveled`
- The background worker drives incremental compaction automatically
- `COMPACT` command still triggers a full compaction
- All tests pass, no clippy warnings
