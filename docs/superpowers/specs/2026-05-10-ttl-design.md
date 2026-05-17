# TTL: Per-Key Time-To-Live for KV and LSM Engines

**Date:** 2026-05-10
**Task:** [#56](../../../TASKS.md) — `TTL` command
**Scope:** Add per-key time-to-live (TTL) support to both engines. Clients can attach an expiry to a key on write (via extended `WRITE`/`MSET`) or post-hoc (via a new `TTL` command). Expired keys are invisible on read and dropped during compaction. Designed primarily for the telemetry experimental use case ([docs/telemetry-store-experiment.md](../../telemetry-store-experiment.md)).
**Out of scope:** Time-Window Compaction Strategy, collections / column families, server-wide default TTL, `KEEPTTL` flag, background expiry sweeper, sub-second TTL precision, TTL inspection commands. All filed as follow-up tasks (see Future Work).

## Problem

The store currently grows unbounded — there is no mechanism for keys to expire. For telemetry workloads (the immediate use case) and for caches (a future use case), this is the single biggest blocker to sustained operation. Writes accumulate indefinitely; the only path to disk reclamation is an explicit `DELETE` from the client.

Production KV stores universally provide per-key TTL (Redis `EXPIRE`, Cassandra `USING TTL`, RocksDB `DBWithTTL`, Bitcask's original record format). The Bitcask paper itself reserves an `expiry_secs` field in its record header — TTL is part of the standard storage repertoire.

## Design

### 1. Storage format change — additive bitmask

The `tombstone: bool` byte in `RecordHeader` ([src/record.rs](../../../src/record.rs)) becomes a `flags: u8`. Two bits are defined; six are reserved for future use.

```rust
pub const FLAG_TOMBSTONE:  u8 = 1 << 0;
pub const FLAG_HAS_EXPIRY: u8 = 1 << 1;

pub struct RecordHeader {
    pub crc32: u32,
    pub key_size: u64,
    pub value_size: u64,
    pub flags: u8,
    pub expiry_ms: Option<u64>,  // logical only; on disk only if FLAG_HAS_EXPIRY
}
```

**On-disk layout:**

```
| crc32(4) | key_size(8) | value_size(8) | flags(1) | [expiry_ms(8) if FLAG_HAS_EXPIRY] | key | value |
```

- **Backward-compatible.** Old records on disk have `flags ∈ {0, 1}` and no trailing expiry. New parsers read them identically — bit 0 (TOMBSTONE) sits where the boolean tombstone byte sat. No migration, no data wipe, no version bump.
- **Zero overhead for keys without TTL.** `flags = 0` produces byte-identical on-disk layout to today.
- **8 bytes per expiring record** for the `expiry_ms: u64` field.
- **CRC payload** covers `key_size || value_size || flags || [expiry_ms] || key || value`. Old segments still verify because their on-disk bytes haven't changed — we only relabel the byte at the tombstone position.
- **Tombstones with TTL** are representable (`flags = 0b11`) but unusual; supported naturally.

`RECORD_HEADER_LEN` (currently 21) stays the same — it remains the size of the *fixed* header. The optional 8-byte expiry is accounted for separately at read time.

This is the encoding the original Bitcask paper uses and the same additive-bit pattern Cassandra, Protocol Buffers, and many real formats use for schema evolution.

### 2. Wire protocol — Option B (extend WRITE/MSET) + new TTL command

Three changes to [src/bffp.rs](../../../src/bffp.rs).

#### 2.1 New `TTL` opcode (post-hoc setter)

`OpCode::Ttl = 12`:

```
| total_len(4) | OpCode::Ttl(1) | key_len(2) | key | seconds(4) |
```

Semantics:

- `seconds > 0`: set `expiry_ms = now_ms() + seconds * 1000` for the key. Response: `Ok`.
- `seconds == 0`: PERSIST — strip any existing expiry. Response: `Ok`.
- Key absent: response `NotFound`.

Maximum `seconds` is `u32::MAX` ≈ 136 years. No artificial cap; we don't have Cassandra/DynamoDB's signed-int storage limitation because `expiry_ms: u64` is safely future-proof.

Mechanically, `TTL key seconds` rewrites the record (LSM: new memtable entry; KV: append new record + index update). Cost is one write.

#### 2.2 Extended `WRITE` frame

Wire-format break to `OpCode::Write = 2`:

```
| total_len(4) | OpCode::Write(1) | flags(1) | key_len(2) | key | value_len(4) | value | [seconds(4) if flags & HAS_TTL] |
```

- `flags & 0x01` = `HAS_TTL`: trailing `seconds(4)` present, server converts to `expiry_ms`.
- Other 7 bits reserved (e.g., for a future `KEEPTTL` flag).

#### 2.3 Extended `MSET` frame

Wire-format break to `OpCode::Mset = 10`. Per-entry flags byte:

```
| total_len(4) | OpCode::Mset(1) | [ flags(1) | key_len(2) | key | value_len(4) | value | [seconds(4) if flags & HAS_TTL] ]* |
```

Each entry has its own flags + optional TTL — mixed-TTL bulk writes possible in one frame.

#### 2.4 Why hard-break the wire format

BFFP has no version byte. Adding one — or repurposing high opcode bits — is more complex than just updating in-repo clients (rustikli, kvbench, redis-compare) atomically in this PR. The project hasn't shipped externally; the cost of "wire compat" is artificial. If/when external clients exist, version negotiation can be added as a real feature.

### 3. Engine trait changes

[src/engine.rs](../../../src/engine.rs) — `set_with_ttl` and `mset_with_ttl` become the primitives; `set` and `mset` become default-impl wrappers:

```rust
pub trait StorageEngine: Send + Sync {
    // ... existing methods unchanged ...

    fn set_with_ttl(
        &self,
        key: &str,
        value: &str,
        expiry_ms: Option<u64>,
    ) -> io::Result<()>;

    fn mset_with_ttl(
        &self,
        items: Vec<(String, String, Option<u64>)>,
    ) -> io::Result<()>;

    fn ttl(&self, key: &str, expiry_ms: Option<u64>) -> io::Result<TtlOutcome>;

    // Default impls — engines don't override
    fn set(&self, key: &str, value: &str) -> io::Result<()> {
        self.set_with_ttl(key, value, None)
    }
    fn mset(&self, items: Vec<(String, String)>) -> io::Result<()> {
        self.mset_with_ttl(
            items.into_iter().map(|(k, v)| (k, v, None)).collect()
        )
    }
}

pub enum TtlOutcome {
    Set,        // expiry_ms = Some(...), key exists, expiry applied
    Persisted,  // expiry_ms = None, key exists; expiry stripped or was already absent on the key
    NotFound,   // key does not exist
}
```

Both engines refactor their existing `set`/`mset` paths under `set_with_ttl`/`mset_with_ttl`. No behavioral change for non-TTL writes.

**The engine speaks one language: absolute `expiry_ms: Option<u64>`.** All three TTL-aware methods (`set_with_ttl`, `mset_with_ttl`, `ttl`) accept the same shape. Wire-protocol concepts — relative seconds, the `0 = PERSIST` sentinel — are converted at the dispatch boundary (see section 10) before reaching the engine. Engines never see raw seconds.

### 4. Memtable changes

[src/memtable.rs](../../../src/memtable.rs) — values gain an expiry slot:

```rust
pub struct MemtableEntry {
    pub value: Option<String>,  // None = tombstone
    pub expiry_ms: Option<u64>,
}

pub struct Memtable {
    entries: BTreeMap<String, MemtableEntry>,
    size_bytes: usize,
}
```

- `entry()` returns `Option<&MemtableEntry>` (was `Option<&Option<String>>`).
- `insert(key, value, expiry_ms)` — new signature.
- `remove(key)` — same; writes `MemtableEntry { value: None, expiry_ms: None }`.
- `entries()` return type changes to `&BTreeMap<String, MemtableEntry>`.
- `size_bytes` accounts for 8 bytes when `expiry_ms.is_some()`.

This cascades through [src/lsmengine.rs](../../../src/lsmengine.rs) — every loop over `memtable.entries()` updates from unwrapping `Option<String>` to reading `entry.value`.

**Overwrite clears TTL.** Per the locked decision, `WRITE` without an explicit TTL clears any existing TTL. This falls out naturally: `set_with_ttl(key, value, None)` calls `memtable.insert(key, value, None)`, which replaces the existing `BTreeMap` slot with `MemtableEntry { value: Some(value), expiry_ms: None }` — overwriting any previous expiry. Implementations must not preserve the old `expiry_ms` on overwrite.

### 5. Read path — lazy expiry

A single helper, called from every read site:

```rust
fn is_expired(expiry_ms: Option<u64>, now_ms: u64) -> bool {
    matches!(expiry_ms, Some(exp) if exp <= now_ms)
}
```

Filtering applies to `get`, `mget`, `exists`, `list_keys` on both engines, and `range` on the LSM. For multi-record paths (`range`, `list_keys`, `mget`), capture `now_ms` once at the start of the call so all entries evaluate against the same wall-clock reading.

**Active memtable expiry semantics:** an expired hit in the active memtable returns `None` and the read does *not* fall through to older SSTables. The active memtable holds the newest version of the key; if that version has expired, the key is gone, even if older segments hold non-expired stale values.

**Stats:** every read returning `None` due to expiry increments `Stats::expired_reads`.

### 6. Compaction-time cleanup

Both engines drop expired records during compaction. Capture `now_ms` once at the start of each compaction pass.

**LSM** — TTL filtering applies in **every SSTable-producing path**, not just `compact_all`:

- [src/sstable.rs](../../../src/sstable.rs) `SSTable::from_memtable` (memtable → SSTable flush)
- `storage_strategy.compact_all` (manual full compaction)
- `storage_strategy.compact_if_needed` invoked via `LsmEngine::compact_step` (the auto-compaction trigger from #35)
- The level-merge paths inside [src/leveled.rs](../../../src/leveled.rs) and the tier-merge paths inside [src/size_tiered.rs](../../../src/size_tiered.rs)

**The safe-drop rule.** At every emit site, when `is_expired(record.expiry_ms, now_ms)`:

> **Drop the record entirely (no tombstone) iff this operation also removes every older version of that key. Otherwise emit a tombstone** (`flags = FLAG_TOMBSTONE`, empty value, no expiry).

Either way, increment `Stats::expired_compacted` per record removed.

**Why a tombstone is sometimes mandatory.** The "expired key is already invisible" claim relies on the section 5 rule that an expired active-memtable hit returns `None` without falling through to older SSTables. That shield only exists *while the entry is in the memtable*. The read path ([src/lsmengine.rs](../../../src/lsmengine.rs) `get`) walks SSTables newest-first and stops at the first hit, where a tombstone (`Some(None)`) shadows older segments. If an emit site that does **not** consume the older versions drops the expired key entirely, the new segment lacks the key, `get` falls through to a lower segment, and a stale, non-expired older value resurrects:

```text
t0   WRITE k v1 (no TTL)        → flushed to a lower SSTable
t1   WRITE k v2 EX 10           → memtable, expiry = t1+10
t12  memtable flushes; v2 expired
       drop v2 entirely (no tombstone)
t13  GET k → not in memtable, new segment has no `k`,
             falls through → returns v1   ← resurrection bug
```

Per-site application of the rule:

| Emit site | Older versions consumed? | Action on expired record |
|---|---|---|
| [src/sstable.rs](../../../src/sstable.rs) `SSTable::from_memtable` (memtable flush) | No — only the memtable is read | **Tombstone** |
| Leveled partial level-merge ([src/leveled.rs](../../../src/leveled.rs)) | Only if the merge includes the bottom-most level holding the key | **Tombstone** unless a lower level still holds an older version; then drop |
| Size-tiered partial tier-merge ([src/size_tiered.rs](../../../src/size_tiered.rs)) | Same conditional as leveled | Same conditional as leveled |
| `storage_strategy.compact_all` (full compaction) | Yes — all segments | **Drop entirely** |
| `storage_strategy.compact_if_needed` via `LsmEngine::compact_step` (#35 auto-trigger) | Per the underlying strategy's merge scope | Drop/tombstone per the partial-merge rows above |

Missing one of these paths means automatic background compaction would never reclaim expired records — implementation must touch all of them.

**KV** ([src/kvengine.rs](../../../src/kvengine.rs) compaction):

- Same safe-drop rule in the segment-merge loop: drop entirely only when the merge consumes every older version of the key; otherwise emit a tombstone.
- Hash-index entries whose only live record was dropped are removed from the index.

### 7. WAL handling

[src/wal.rs](../../../src/wal.rs):

- `Wal::append(key, value, tombstone, expiry_ms)` — new signature carrying the optional expiry. The on-disk WAL record uses the same `Record` format as segment records; no separate WAL schema.
- `Wal::replay()` parses `record.header.expiry_ms` from each record and threads it into `Memtable::insert(key, value, expiry_ms)`. No expiry filtering during replay — load records as-is. Two reasons:
  1. Lazy read-time filter handles it correctly anyway.
  2. Replay shouldn't depend on wall-clock time. A clock skew or NTP correction at startup shouldn't change the in-memory state immediately after replay.

### 8. Time source

```rust
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before UNIX epoch")
        .as_millis() as u64
}
```

Wall clock, called at the engine's edge (`set_with_ttl`, `get`, compaction). No `Clock` trait abstraction in v1. Tests use short TTLs (1 second on the wire) with `thread::sleep(Duration::from_millis(2000))` for safety margin against CI scheduler jitter.

If future tests need clock control (e.g., long-horizon TTLs, exact-boundary behavior), retrofitting a `Clock` trait is a 2–3 hour refactor — file as a follow-up if needed.

### 9. Stats

[src/stats.rs](../../../src/stats.rs) gains:

```rust
pub expired_reads: AtomicU64,       // read returned None due to expiry
pub expired_compacted: AtomicU64,   // record dropped during compaction
```

Both surfaced in the `STATS` command output.

### 10. Server dispatch

[src/server/dispatch.rs](../../../src/server/dispatch.rs) — wherever `Command::Write` and `Command::Mset` are dispatched today, route through `set_with_ttl`/`mset_with_ttl` with the parsed expiry. New `Command::Ttl(key, seconds)` variant dispatches to `engine.ttl(&key, expiry_ms)`.

`Command` enum changes:

```rust
pub enum Command {
    // ... existing ...
    Write(String, String, Option<u32>),       // was (String, String)
    Mset(Vec<(String, String, Option<u32>)>), // was (Vec<(String, String)>)
    Ttl(String, u32),                         // new
}
```

The `Option<u32>` carries seconds-from-the-wire. **The dispatch layer converts to absolute `Option<u64>` ms before calling the engine** — engines never see raw seconds or the `0 = PERSIST` sentinel:

```rust
fn seconds_to_expiry_ms(seconds: u32) -> Option<u64> {
    if seconds == 0 {
        None  // PERSIST sentinel — strip expiry
    } else {
        Some(now_ms() + (seconds as u64) * 1000)
    }
}

// dispatch sites
Command::Write(key, value, ttl_seconds) => {
    let expiry_ms = ttl_seconds.and_then(|s| {
        if s == 0 { None } else { Some(now_ms() + (s as u64) * 1000) }
    });
    engine.set_with_ttl(&key, &value, expiry_ms)
}

Command::Ttl(key, seconds) => {
    let expiry_ms = seconds_to_expiry_ms(seconds);
    engine.ttl(&key, expiry_ms)
}
```

Note: `Command::Ttl` keeps `u32` seconds (matches the wire). The `0 = PERSIST` rule lives at this layer, not in the engine.

## Test Plan

New test files in [tests/](../../../tests/):

| File | Coverage |
|---|---|
| `tests/record_ttl.rs` | Record encode/decode with `FLAG_HAS_EXPIRY` (round-trip, CRC validity, mixed flag combinations including tombstone+expiry) |
| `tests/memtable_ttl.rs` | Memtable insert/lookup with expiry, `size_bytes` tracking, tombstone-with-expiry edge case |
| `tests/bffp_ttl.rs` | Wire encode/decode for new `TTL` opcode, extended `WRITE`/`MSET` frames, `flags` byte handling, mixed-TTL `MSET` round-trips |
| `tests/lsm_ttl.rs` | LSM end-to-end: set-with-TTL, get pre/post expiry, range filters expired, compaction drops expired, WAL replay preserves expiry |
| `tests/kv_ttl.rs` | KV end-to-end: same scenarios as LSM, plus hash-index cleanup on compaction |
| `tests/ttl_command.rs` | `TTL` command: existing key → `Set`, `seconds=0` → `Persisted`, missing key → `NotFound`; `WRITE` without TTL on a key with TTL clears the TTL |

Existing tests in `tests/lsmengine.rs`, `tests/sstable.rs`, etc. are updated where they touch `MemtableEntry` or `RecordHeader` field names. No behavioral expectations change for non-TTL paths.

All TTL-bound tests use 1-second wire TTLs and 2-second sleeps for safety margin. Estimated total test-suite runtime increase: 15–30 seconds.

## Migration

None for users — old segment files are read by the new parser unchanged.

For in-repo clients (rustikli, kvbench, redis-compare): the BFFP encoder for `WRITE` and `MSET` is updated to emit the new flags byte. Old-format frames are no longer accepted by the server. This is a hard break; clients are updated atomically in the same PR.

## Future Work (filed as follow-up tasks after this PR lands)

Listed in the order they will be filed and worked. Numbers are sequential per `AGENTS.md`; the first four form the planned arc toward telemetry-grade retention, the rest are independent improvements.

| # | Task | Notes |
|---|---|---|
| #74 | Server-side aggregation (`SUM`/`AVG`/`MIN`/`MAX` over `RANGE`) | Small, telemetry-aligned, independent of TTL. Worked first. |
| #75 | Collections / column families (RocksDB-style) | Per-collection memtable, SSTables, compaction config, default TTL. Major architectural addition; the keystone for what follows. |
| #76 | Per-collection default TTL | Trivial once #75 exists. Replaces any need for a server-wide `--default-ttl`. |
| #77 | Time-Window Compaction Strategy (TWCS) | Per agent research: requires collections to be meaningfully implemented (operates on a per-collection SSTable set). |
| #78 | `KEEPTTL` flag on `WRITE`/`MSET` | One reserved flag bit; preserves existing TTL on overwrite |
| #79 | Background expiry sweeper (Redis-style sampling) | Only if compaction-driven cleanup proves insufficient |
| #80 | TTL inspection commands (`PTTL`, `EXPIRETIME`) | Read remaining TTL or absolute expiry |
| #81 | `Clock` trait abstraction for testable time-dependent logic | Only if long-horizon or boundary-condition tests become necessary |

A server-wide `--default-ttl` flag was considered and rejected. Per agent research, prefix-based or server-wide defaults are config sugar that don't unlock LSM lessons. The retention-by-default story is told properly through collections (#75) plus per-collection defaults (#76).

## Decisions Locked

| Decision | Choice | Rationale |
|---|---|---|
| Engine scope | Both KV and LSM | Matches MSET-style coverage; both engines benefit from telemetry workloads |
| Wire protocol | Option B (extend WRITE/MSET + new TTL command) | Atomic SET-with-TTL, single-frame mixed-TTL bulk writes; matches modern Redis trajectory |
| Storage encoding | Bitmask flags byte (additive) | Backward-compatible, zero migration, zero overhead for non-TTL records |
| Time encoding | `u64` ms on disk, `u32` seconds on wire | ms gives future-proof storage; seconds matches the command-line TTL semantics and Bitcask paper |
| `TTL key 0` | PERSIST (strip expiry) | Matches Redis `PERSIST` semantics; least surprise |
| Overwrite semantics | `WRITE` without TTL clears existing TTL | Redis default; predictable. `KEEPTTL` flag deferred |
| Background sweeper | None | LSMs with active compaction don't need it (Cassandra, RocksDB, Bitcask precedent) |
| Clock abstraction | None in v1 | Tests use real time; abstraction is a 2–3 hr retrofit if needed later |
| TTL upper bound | `u32::MAX` seconds (~136 years) | Natural wire limit; no Cassandra/DynamoDB-style cap needed |
