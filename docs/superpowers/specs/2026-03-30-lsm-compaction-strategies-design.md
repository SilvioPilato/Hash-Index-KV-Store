# LSM Compaction Strategies Design

**Date:** 2026-03-30
**Task:** #27 — Leveled compaction
**Status:** Approved

---

## Overview

Replace the current flat `compact()` implementation in `LsmEngine` (which merges all segments into a single SSTable) with a pluggable compaction strategy system. Two strategies are supported: **size-tiered** (default, improves on current behavior) and **leveled** (LevelDB-style, with level-based no-overlap guarantees).

The strategy is selected at startup via a CLI flag and encapsulated behind a `StorageStrategy` trait, keeping `LsmEngine` clean and strategy-agnostic.

---

## Architecture

### `LsmEngine` changes

Replace `segments: Vec<SSTable>` with `strategy: Box<dyn StorageStrategy>`.

```
LsmEngine {
    memtable: Memtable,
    wal: Wal,
    db_path: String,
    db_name: String,
    max_memtable_bytes: usize,
    strategy: Box<dyn StorageStrategy>,   // ← new
}
```

- `compact()` → flushes the live memtable to an SSTable first (matching current behavior), then calls `strategy.compact_all()`. This preserves the existing semantic: COMPACT always produces a fully compacted on-disk state.
- Background worker → calls `strategy.compact_if_needed()` via a write lock on the engine (see Integration section).
- `get()` → uses `strategy.iter_for_key(key)` instead of `self.segments.iter().rev()`.
- `range()` → uses `strategy.iter_files_for_range(start, end)` (see RangeScan section).
- `from_dir()` → uses a `load_strategy(dir, name, strategy_type, config)` factory that reads existing files and builds the correct strategy.

### `StorageStrategy` trait (`src/compaction.rs`)

```rust
pub trait StorageStrategy: Send + Sync {
    /// Called after every memtable flush.
    fn add_sstable(&mut self, sst: SSTable);

    /// Called by the background worker. Runs at most one compaction step.
    /// Returns true if compaction ran. Deliberately single-step: if multiple
    /// levels need compaction, the worker will call this again on the next tick.
    fn compact_if_needed(&mut self, db_path: &str, db_name: &str) -> io::Result<bool>;

    /// Full compaction of all on-disk files. Called by COMPACT command (after
    /// LsmEngine has already flushed the live memtable).
    fn compact_all(&mut self, db_path: &str, db_name: &str) -> io::Result<()>;

    /// Returns SSTables to check for a given key, in priority order (newest first).
    fn iter_for_key<'a>(&'a self, key: &str) -> Box<dyn Iterator<Item = &'a SSTable> + 'a>;

    /// Returns files whose key range overlaps [start, end], for range scans.
    /// Files must expose their min/max key so callers can prune non-overlapping files.
    fn iter_files_for_range<'a>(
        &'a self,
        start: &str,
        end: &str,
    ) -> Box<dyn Iterator<Item = &'a SSTable> + 'a>;

    /// Returns all SSTables for list_keys.
    fn iter_all<'a>(&'a self) -> Box<dyn Iterator<Item = &'a SSTable> + 'a>;

    /// Total number of SSTable files across all levels/tiers.
    /// LsmEngine delegates StorageEngine::segment_count() to this method.
    fn segment_count(&self) -> usize;
}
```

**Note on `segment_count` naming:** `StorageEngine` already has `fn segment_count(&self) -> usize`. `LsmEngine` implements `StorageEngine::segment_count` by delegating to `strategy.segment_count()`. Both traits define the same method name but they are different traits; there is no collision — the implementer does not need two separate implementations.

**Note on `compact_all` implementation:** Implementations of `compact_all` are not required to delegate to `compact_if_needed` internally. `LeveledCompaction::compact_all` loops over `compact_if_needed` until no level needs compaction. `SizeTieredCompaction::compact_all` performs a direct full merge in a single operation. Both satisfy the trait contract.

### Background worker integration

The background worker (from task #35) holds an `Arc<RwLock<Box<dyn StorageEngine>>>`. It acquires a write lock and calls `engine.compact_step()` directly on the `Box<dyn StorageEngine>` — no downcast is needed because `compact_step` is a trait method with a default no-op.

Add to `StorageEngine` trait:

```rust
fn compact_step(&mut self) -> io::Result<bool> { Ok(false) }  // default: no-op
```

`LsmEngine` overrides this to call `strategy.compact_if_needed(...)`. `KVEngine` inherits the default no-op.

### RangeScan integration

`LsmEngine` implements `RangeScan::range()`. After the refactor, instead of iterating `self.segments` directly, it calls `strategy.iter_files_for_range(start, end)` to get only files whose key range overlaps `[start, end]`. The existing key-range pruning logic (checking `get_min()` / `get_max()`) moves into the strategy implementations.

---

## Size-Tiered Compaction (`src/size_tiered.rs`)

Default strategy. Improves on the current behavior (which was always a full merge) by only compacting groups of similarly-sized files.

### Struct

```rust
pub struct SizeTieredCompaction {
    files: Vec<SSTable>,   // newest-to-oldest
    config: SizeTieredConfig,
}

pub struct SizeTieredConfig {
    pub min_threshold: usize,   // min files per bucket to trigger (default: 4)
    pub max_threshold: usize,   // max files per bucket before forcing (default: 32)
    pub bucket_low: f64,        // default: 0.5 (hardcoded, not exposed as CLI flag)
    pub bucket_high: f64,       // default: 1.5 (hardcoded, not exposed as CLI flag)
}
```

`bucket_low` and `bucket_high` are not exposed as CLI flags — they are hardcoded defaults. This is intentional: they are tuning knobs that would add CLI noise without educational value.

### Bucketing logic

1. When a new SSTable arrives (via `add_sstable`), search existing buckets for a size match.
2. A file matches a bucket if: `bucket_avg * bucket_low <= file_size <= bucket_avg * bucket_high` (where `bucket_avg` is the average size of files already in the bucket).
3. If multiple buckets match, choose the one with the smallest size difference (`|bucket_avg - file_size|`) — best-fit strategy.
4. If no bucket matches, create a new bucket with just this file.
5. If a bucket reaches `>= min_threshold` files → trigger compaction of that bucket → merge all files into a single new SSTable.
6. Compaction runs in `compact_if_needed` at most once per call.

**Output file size:** Size-tiered compaction produces a single SSTable per bucket merge, regardless of output size. For large buckets this can produce large files — this is a known trade-off of the strategy and acceptable for this educational implementation.

### File naming

Uses the existing format: `{name}_{timestamp}.sst` — no change.

### `compact_all`

Merges all on-disk files into a single SSTable (matching current behavior, used by COMPACT command after memtable flush).

### Read path

`iter_for_key` and `iter_files_for_range` return files newest-to-oldest — same as current. No overlap guarantee; reads may check multiple files.

### Trade-offs

- ✅ Low write amplification — only similarly-sized files are merged together
- ✅ Files grow progressively larger across rounds of compaction
- ❌ No overlap guarantee — reads may need to check multiple files
- ❌ Higher space amplification — stale data lives longer before being compacted
- ❌ Output files can become large (no per-file size cap)

---

## Leveled Compaction (`src/leveled.rs`)

LevelDB-style. Files at L1+ have non-overlapping key ranges. L0 files (flushed directly from memtable) may overlap.

### `Level` struct

```rust
pub struct Level {
    pub level_num: usize,
    pub max_bytes: u64,
    pub files: Vec<SSTable>,   // sorted by min_key for L1+; insertion order for L0
}

impl Level {
    pub fn total_bytes(&self) -> u64;
    pub fn needs_compaction(&self, l0_threshold: usize) -> bool;
    // L0: files.len() >= l0_threshold
    // L1+: total_bytes > max_bytes

    pub fn overlapping_files(&self, min_key: &str, max_key: &str) -> Vec<&SSTable>;
    // Returns files whose key range overlaps [min_key, max_key]
}
```

### `LeveledCompaction` struct

```rust
pub struct LeveledCompaction {
    levels: Vec<Level>,   // always exactly max_levels levels (L0..L{max_levels-1})
    config: LeveledConfig,
}

pub struct LeveledConfig {
    pub l0_compaction_threshold: usize,  // default: 4
    pub l1_max_bytes: u64,               // default: 10 MB
    pub level_multiplier: u64,           // default: 10
    pub max_levels: usize,               // default: 4
    pub max_file_bytes: u64,             // max size per output SSTable (default: 2 MB)
}
```

`max_bytes` for each level: `l1_max_bytes * level_multiplier^(level_num - 1)`.

### File naming

`{name}_L{level}_{timestamp}.sst` — level encoded in filename so startup can reconstruct levels without a manifest.

**Constraint:** Output files from a compaction must be written directly with the target-level name (e.g., `mydb_L1_<ts>.sst`). Files are never renamed after the fact. This is required because the manifest-free design relies on filenames to determine level membership on restart.

**New parser required:** The existing `SSTable::parse(filename)` in `src/sstable.rs` uses `rsplit_once('_')` and would incorrectly parse `mydb_L1_1711234567.sst` as name=`mydb_L1`, timestamp=`1711234567`. A new function `SSTable::parse_leveled(filename) -> Option<(SSTable, usize)>` must be added that splits on `_L{n}_` to extract name, level, and timestamp correctly. This function returns a stub SSTable (same as `parse`); the caller must invoke `rebuild_index()` before use.

**`rebuild_index` visibility:** `SSTable::rebuild_index` is currently a private method. It must be changed to `pub(crate)` so that `src/leveled.rs` can call it during startup reconstruction. Add this to the `src/sstable.rs` changes.

**Filename mismatch during `load_strategy`:** If the data directory contains files that do not match the expected pattern for the selected strategy (e.g., flat `{name}_{timestamp}.sst` files found when using leveled strategy, or `{name}_L{n}_{timestamp}.sst` files found when using size-tiered), those files are silently ignored — the same behavior as the existing `get_sstables`, which already filters by `db_name`. Mixing strategies in the same directory is not supported; users should use separate directories per strategy.

**Timestamp collision for multi-file output:** When a single compaction step produces multiple output files (because merged data exceeds `max_file_bytes`), each file must receive a distinct timestamp. Use a monotonic per-compaction sequence counter appended to the nanosecond timestamp: `{name}_L{level}_{timestamp}_{seq}.sst`. This avoids the `create_new(true)` `AlreadyExists` error that would otherwise occur when two files are created within the same nanosecond.

### Compaction: L0 → L1

Triggered when `L0.files.len() >= l0_compaction_threshold`.

1. Compute the key range covered by all L0 files: `[min_of_all_L0_mins, max_of_all_L0_maxs]`.
2. Find L1 files overlapping that range via `L1.overlapping_files(min, max)`.
3. Merge-sort all L0 files + overlapping L1 files, keeping newest value per key. **Tombstones are preserved** unless this is the final level (L{max_levels-1}) — dropping a tombstone at L0→L1 would allow stale values in deeper levels (L2+) to resurface on reads.
4. Write output as one or more new L1 files, each up to `max_file_bytes`. Files are sorted by key and have non-overlapping ranges, all named `{name}_L1_{timestamp}.sst`.
5. Insert new files into L1 in key-sorted order. Remove old L0 files and replaced L1 files from disk and from the in-memory level.

### Compaction: Ln → Ln+1 (L1+)

Triggered when `Ln.total_bytes() > Ln.max_bytes`.

1. Pick the first file from Ln (simplest selection; LevelDB uses a round-robin pointer, which is optional here).
2. Find overlapping files in Ln+1 via `overlapping_files`.
3. Merge-sort picked file + overlapping Ln+1 files, keeping newest value per key. **Tombstones are preserved** unless Ln+1 is the final level.
4. Write output as new Ln+1 files (up to `max_file_bytes` each), named `{name}_L{n+1}_{timestamp}.sst`.
5. Update both levels: remove old files from disk and from in-memory level structs, insert new files in key-sorted order.

### Tombstone rule (summary)

> Tombstones are dropped during compaction **only when merging into the final level** (`level_num == max_levels - 1`). At all other levels, tombstones are written to output files to prevent stale values in deeper levels from becoming visible.

### `compact_if_needed`

Scans levels from L0 outward. Runs **at most one compaction step** (one level-pair) per call, then returns `true`. The background worker calls this repeatedly on each tick. This is a deliberate design choice: single-step keeps the write lock held for a short time and matches LevelDB's compaction scheduling model.

### `compact_all`

Runs compaction steps in a loop until no level needs compaction (i.e., `compact_if_needed` returns `false`).

### Read path

- **L0**: check all files newest-to-oldest (they may overlap).
- **L1+**: binary search on sorted key ranges — at most 1 file per level can contain the key.

`iter_for_key(key)` returns: L0 files (newest-to-oldest) + for each L1..L{n}: the single file whose range contains the key (if any).

`iter_files_for_range(start, end)` returns: all L0 files + for each L1+: files overlapping `[start, end]`.

### Trade-offs

- ✅ No overlap at L1+ → reads check at most 1 file per level
- ✅ Lower space amplification — stale data is overwritten quickly
- ❌ Higher write amplification — a key may be rewritten many times as it moves through levels
- ❌ More complex compaction logic

---

## CLI Flags (`src/settings.rs`)

```
--compaction-strategy leveled|size-tiered   (default: size-tiered)

# Leveled-specific:
--l0-compaction-threshold N                 (default: 4)
--l1-max-bytes N                            (default: 10485760)
--level-multiplier N                        (default: 10)
--max-levels N                              (default: 4)
--max-file-bytes N                          (default: 2097152)

# Size-tiered-specific:
--st-min-threshold N    (default: 4)
--st-max-threshold N    (default: 32)
# bucket_low (0.5) and bucket_high (1.5) are hardcoded constants, not CLI flags.
```

---

## New Files

| File | Purpose |
|------|---------|
| `src/compaction.rs` | `StorageStrategy` trait |
| `src/leveled.rs` | `LeveledCompaction`, `Level`, `LeveledConfig` |
| `src/size_tiered.rs` | `SizeTieredCompaction`, `SizeTieredConfig` |
| `tests/leveled_compaction.rs` | Tests for leveled strategy |
| `tests/size_tiered_compaction.rs` | Tests for size-tiered strategy |

## Modified Files

| File | Change |
|------|--------|
| `src/lsmengine.rs` | Replace `segments: Vec<SSTable>` with `strategy: Box<dyn StorageStrategy>`; add `compact_step()` |
| `src/sstable.rs` | Add `SSTable::parse_leveled(filename)` for leveled filename format; change `rebuild_index` to `pub(crate)` |
| `src/engine.rs` | Add `compact_step(&mut self) -> io::Result<bool>` with default no-op |
| `src/settings.rs` | Add compaction strategy CLI flags |
| `src/main.rs` | Wire new settings to `LsmEngine` construction; update background worker to call `compact_step()` |

---

## Test Plan

### `tests/leveled_compaction.rs`

- After L0→L1 compaction, L1 files have non-overlapping key ranges
- L0→L1 compaction producing multiple output files: verify all output files also have non-overlapping key ranges
- A `get` returns the most recent value for a key after compaction
- A key deleted in L0 (tombstone) is not visible even when the same key exists in L2 — tombstone is not dropped prematurely
- Deletes (tombstones) are dropped only at the final level during compaction
- L0 compaction trigger fires at `l0_compaction_threshold` files
- Ln→Ln+1 compaction trigger fires when level exceeds `max_bytes`
- `compact_all` leaves no level needing compaction
- Restart/recovery: write files, restart engine, verify correct level reconstruction from filenames and correct read results
- `iter_for_key` returns the newest value when a key appears in multiple files across levels

### `tests/size_tiered_compaction.rs`

- Files of similar size are grouped into the same bucket
- Files outside the bucket range (`< avg * 0.5` or `> avg * 1.5`) are not included in a bucket
- A bucket with `< min_threshold` files does not trigger compaction
- A bucket with `>= min_threshold` files triggers compaction; stale keys are removed from output
- Reads return correct values after multiple rounds of compaction
- `compact_all` merges everything into a single file
- `compact_all` followed by engine restart produces identical read results (round-trip durability)
- `iter_for_key` returns the newest value when a key appears in multiple files
