# Engine Concurrency: Internal Locking and Write Buffering

**Date:** 2026-04-05
**Scope:** Writer-reader contention and write throughput for both KVEngine and LsmEngine. Compaction blocking is out of scope (separate task).

## Problem

The entire storage engine is wrapped in a single `Arc<RwLock<Box<dyn StorageEngine>>>` in `main.rs`. All I/O — disk reads, writes, flushes — happens while holding this lock. Consequences:

- **KV writes are slow under lock**: every `set()` does a disk append + fsync while holding the exclusive write lock (~3.5ms per write, ~1155 ops/sec regardless of thread count).
- **LSM memtable flush blocks everything**: when a `set()` triggers the size threshold, the full SSTable serialization happens synchronously under the write lock.
- **Readers wait for writers**: any pending read blocks behind a write, even though reads are logically independent.

## Design

### 1. StorageEngine Trait: Interior Mutability

Change all mutating methods from `&mut self` to `&self`:

```rust
pub trait StorageEngine: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<(String, String)>, io::Error>;
    fn set(&self, key: &str, value: &str) -> Result<(), io::Error>;
    fn delete(&self, key: &str) -> Result<Option<()>, io::Error>;
    fn compact(&self) -> Result<(), io::Error>;
    fn compact_step(&self) -> io::Result<bool> { Ok(false) }  // default preserved
    fn dead_bytes(&self) -> u64;
    fn total_bytes(&self) -> u64;
    fn segment_count(&self) -> usize;
    fn list_keys(&self) -> io::Result<Vec<String>>;
    fn exists(&self, key: &str) -> bool;
    fn mget(&self, keys: Vec<String>) -> Result<Vec<(String, Option<String>)>, io::Error>;
    fn mset(&self, keys: Vec<(String, String)>) -> Result<(), io::Error>;
    fn as_any(&self) -> &dyn Any;
}
```

The `RangeScan` trait's `range(&self, ...)` already takes `&self` — no signature change needed. Its lock protocol is defined in Section 3 below.

Each engine uses `RwLock`/`Mutex` on internal fields to manage its own concurrency.

### 2. main.rs: Drop the Global Lock

`Arc<RwLock<Box<dyn StorageEngine>>>` becomes `Arc<dyn StorageEngine>`. The TCP handler calls `db.get()` / `db.set()` directly — no lock acquisition at the call site. All `database.read().unwrap()` / `database.write().unwrap()` calls are removed.

**`maybe_trigger_compaction`** changes accordingly: it no longer acquires a global lock. It calls `db.dead_bytes()` / `db.total_bytes()` / `db.segment_count()` directly (these are read-only and lock internally). The background compaction thread calls `db.compact()` directly — `compact()` acquires its own internal locks. The `compacting` atomic flag and `Stats` tracking remain unchanged.

### 3. LSM Engine: Double-Buffered Memtables

**Internal structure:**

```rust
pub struct LsmEngine {
    active: RwLock<Memtable>,
    immutable: RwLock<Option<Memtable>>,
    storage_strategy: RwLock<Box<dyn StorageStrategy>>,
    wal: Mutex<Wal>,
    // config fields unchanged: db_path, db_name, max_memtable_bytes
}
```

**Lock ordering (must always be acquired in this order):**

1. `wal` (Mutex)
2. `active` (RwLock)
3. `immutable` (RwLock)
4. `storage_strategy` (RwLock)

No code path acquires these in reverse order. Locks are released before re-acquiring at the same level.

**Write path (`set`):**

1. Lock `wal` (Mutex) — append entry.
2. Lock `active` (write) — insert into memtable.
3. If memtable exceeds threshold:
   - Lock `immutable` (write) — if already occupied, block until background flush clears it (backpressure).
   - Swap: `let old = std::mem::replace(&mut *active_guard, Memtable::new()); *immutable_guard = Some(old);`
   - Release both locks.
   - Reset WAL.
   - Spawn background flush thread.

The write lock on `active` is held only for the in-memory insert (~microseconds).

**Background flush thread:**

1. Lock `immutable` (read) — serialize to SSTable on disk.
2. Release `immutable` read lock.
3. Lock `storage_strategy` (write) — add new SSTable.
4. Lock `immutable` (write) — set to `None`.

Note: the `immutable` read lock is released before acquiring `storage_strategy` write, then `immutable` write is acquired separately. No lock upgrades.

**Read path (`get`):**

1. Lock `active` (read) — check memtable.
2. Lock `immutable` (read) — check frozen memtable if present.
3. Lock `storage_strategy` (read) — iterate SSTables newest-to-oldest with bloom filter checks.

Each lock is acquired and released independently. All read locks — multiple readers proceed in parallel. Since all three are read locks, acquisition order does not matter for deadlock safety (only read-vs-write or write-vs-write orderings can deadlock). The canonical ordering is preferred by convention but not required.

**Range scan (`range` on `RangeScan` trait):**

Same lock protocol as `get` — acquires `active` (read), `immutable` (read), `storage_strategy` (read). Each lock acquired and released independently. Results merged from all three sources. Same deadlock-safety note as `get` applies — read-only paths are order-independent.

**`delete` — TOCTOU note:** `delete()` calls `self.get()` to check existence, then writes a tombstone. Between the read and write, another writer could modify the key. This is acceptable: a tombstone for a non-existent key is harmless in an LSM (compaction drops it). No atomicity guarantee needed.

**`list_keys`:** Acquires `active` (read), `immutable` (read), and `storage_strategy` (read). Since all three are read locks, acquisition order does not matter for deadlock safety — `RwLock` read locks cannot deadlock with each other (only read-vs-write or write-vs-write orderings can deadlock). The canonical ordering is preferred by convention but not required. Since reads are independent snapshots, a concurrent flush could cause a key to appear in both or neither — this is acceptable for `list_keys` which is informational, not transactional.

**`compact`:** Acquires `wal` → `active` (write) → `storage_strategy` (write). All three locks are held simultaneously for the duration of the compact operation — WAL is not released and re-acquired. Under these locks: flush memtable to SSTable, clear memtable, reset WAL, run `compact_all` on the strategy. Full compaction redesign is out of scope, but this minimal lock discipline prevents deadlocks.

### 4. KV Engine: Write Buffer with WAL

**Internal structure:**

```rust
pub struct KVEngine {
    buffer: RwLock<HashMap<String, Option<String>>>,  // pending writes (None = tombstone)
    index: RwLock<HashIndex>,
    wal: Mutex<Wal>,
    active_file: Mutex<File>,
    active_segment: Mutex<Segment>,
    // Atomics (no lock needed):
    //   dead_bytes: AtomicU64
    //   total_bytes: AtomicU64
    //   segment_count: AtomicUsize
    //   writes_since_fsync: AtomicU64
    // Config (immutable after construction):
    //   db_path, db_name, max_segment_bytes, fsync_strategy
    // Owned by active_file Mutex (accessed only when Mutex held):
    //   fsync_handle: Option<BackgroundWorker>
}
```

**Mutable bookkeeping fields:**

- `dead_bytes`, `total_bytes`, `segment_count`, `writes_since_fsync` — become `AtomicU64`/`AtomicUsize`. Updated with `Relaxed` ordering (counters, not coordination).
- `active_segment` — wrapped in its own `Mutex`, acquired alongside `active_file` during flush and roll.
- `fsync_handle` — logically owned by whoever holds `active_file` Mutex. Stored alongside or guarded by the same Mutex scope.

**Lock ordering:**

1. `wal` (Mutex)
2. `buffer` (RwLock)
3. `active_file` + `active_segment` (Mutex, always together)
4. `index` (RwLock)

**Write path (`set`):**

1. Lock `wal` (Mutex) — append entry.
2. Lock `buffer` (write) — insert key/value.
3. If buffer exceeds threshold:
   - Drain buffer under write lock (collect entries, clear buffer).
   - Release `buffer` write lock.
   - Lock `active_file` + `active_segment` (Mutex) — append all entries to segment, handle roll if needed.
   - Lock `index` (write) — update hash index with new offsets.
   - Reset WAL.
   - Update atomics (`total_bytes`, `dead_bytes`, `segment_count`).

Write lock on `buffer` is held for the in-memory insert (~microseconds). Flush is batched.

**Read path (`get`):**

1. Lock `buffer` (read) — check for key. If found (value or tombstone), return.
2. Lock `index` (read) — hash lookup for offset. If not found, return None.
3. Open an independent file descriptor via `File::open()`, seek, read record. File I/O happens after releasing the index lock — only the offset is needed. Readers never touch `active_file`. This is a critical safety property: readers must always open their own file descriptors (not `try_clone()` from `active_file`), because cloned descriptors share the seek offset and would race under concurrent reads.

Readers never wait for writers on the hot path. The only contention is during buffer flush when `index` write lock is briefly held (fast in-memory HashMap update, not disk I/O).

**Flush batching:** Instead of one disk write per `set()` (~3.5ms), N writes batch into one sequential append. With 100 entries per flush, that's ~100x fewer fsyncs.

**WAL reuse:** Reuses the existing `Wal` struct from the LSM engine. Same format, same replay logic. On crash recovery, WAL replays into the buffer, then flushes.

**O(1) reads preserved:** The hash index is unchanged. The buffer is checked first (O(1) HashMap lookup), then the hash index (O(1)). Asymptotic complexity is identical.

**`compact` / `build_compacted`:** `build_compacted` creates a local, unshared `KVEngine` instance whose internal locks are never contended by other threads. It acquires `index` (read) on `self` to collect keys, calls `self.get()` for each key (which acquires `buffer` read then `index` read — both released per-call), and writes into the local engine. Once the new engine is built, old segment files are deleted. Then the final swap into `self` requires: flush `buffer` first, then acquire `active_file` + `active_segment` (Mutex) and `index` (write) simultaneously to replace the internals. Concurrent readers that still have open file handles to deleted segments are safe — the OS keeps the file alive until all handles are closed. Full compaction redesign is out of scope.

### 5. Consistency Semantics for Bulk Operations

**`mset`:** Each individual `set()` acquires and releases its own locks. A concurrent reader may see a partial `mset` (some keys updated, others not). This is acceptable — `mset` is a throughput optimization (one round trip), not a transactional guarantee.

**`mget`:** Each individual `get()` acquires and releases its own locks. Concurrent writes between gets could yield different snapshot views across keys. This is acceptable — same semantics as issuing N individual `GET` commands.

**`list_keys`:** Informational, not transactional. May reflect a slightly stale or inconsistent view during concurrent writes. Acceptable.

## What Changes

| Component | Change |
|-----------|--------|
| `StorageEngine` trait | `&mut self` → `&self` on `set`, `delete`, `compact`, `compact_step`, `mset` |
| `RangeScan` trait | No signature change (already `&self`); lock protocol defined |
| `main.rs` | `Arc<RwLock<Box<dyn StorageEngine>>>` → `Arc<dyn StorageEngine>`, remove all lock acquisition from TCP handler, update `maybe_trigger_compaction` |
| `KVEngine` | Add write buffer, WAL, internal `RwLock`/`Mutex`, atomics for counters |
| `LsmEngine` | Add immutable memtable, internal `RwLock`/`Mutex`, background flush thread |

## What Doesn't Change

- `Memtable`, `SSTable`, `HashIndex`, `Record`, `Wal` — remain single-threaded, used only while holding the appropriate lock.
- `StorageStrategy` trait and implementations (Leveled, SizeTiered) — `&mut self` methods accessed through `RwLock` write; `&self` methods through read.
- Binary protocol (`bffp`), TCP framing, command parsing.
- `Stats` — already uses atomics.
- `BackgroundWorker` — unchanged.
- On-disk format — segment files, hint files, WAL format.

## Testing

- Existing tests continue to work (single-threaded correctness unchanged).
- Add concurrent stress tests: multiple writers + readers hitting the engine simultaneously.
- Test WAL replay for KV engine (crash recovery with buffered writes).
- Test immutable memtable backpressure (flush slower than writes).
- Benchmark before/after with kvbench to measure improvement.
