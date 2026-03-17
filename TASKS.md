# In Progress

# Open Tasks

## #14 — Hardcoded port in integration tests

Use `mmap` to memory-map SSTable files instead of reading via file I/O. Combined with a sparse index, lookups become pointer arithmetic + memcmp with no syscalls. Explore platform-specific considerations and safety tradeoffs.

# Closed Tasks

<!-- Move completed tasks here to keep a reference of what was done. -->

## #19 — Bloom filter for key existence (DDIA Ch. 3)

Once there are multiple segments (from #16 or #18), checking every segment for a missing key is expensive. A per-segment **Bloom filter** lets you skip segments that definitely don't contain the key. Implementing one from scratch (bit array + k hash functions) is a good exercise in probabilistic data structures, directly referenced in DDIA's LSM-Tree discussion.

## #18 — Simple SSTable / sorted segments (DDIA Ch. 3)

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/17 segment format as a second storage engine alongside the existing Bitcask-style KVEngine.

**Architecture changes:**
- Extracted `StorageEngine` trait (`src/engine.rs`) with `get`, `set`, `delete`, `compact` + `Send + Sync` supertraits.
- Existing Bitcask DB renamed to `KVEngine` (`src/kvengine.rs`), implements `StorageEngine`.
- Added `--engine kv|lsm` CLI flag; `main.rs` uses `Box<dyn StorageEngine>` for runtime polymorphism.

**LSM implementation:**
- `Memtable` (`src/memtable.rs`): in-memory `BTreeMap<String, Option<String>>` with size tracking, tombstones, flush threshold.
- `SSTable` (`src/sstable.rs`): sorted segment files using the existing `Record` format. Sparse index (sampled every 64 keys) with `partition_point` binary search for fast offset-based lookups. BufReader for buffered I/O.
- `LsmEngine` (`src/lsmengine.rs`): wires memtable + SSTable segments. Reads check memtable first (distinguishing tombstones from missing), then segments newest-to-oldest. Compaction merge-sorts all segments + memtable, drops tombstones.

**Tests:** 36 new tests (13 memtable, 9 sstable, 14 lsmengine). Total: 83 tests passing.

## #23 — Background thread/timer infrastructure

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/16

Added a `BackgroundWorker` struct (`src/worker.rs`) that spawns a thread with a configurable tick interval, runs a job each tick via `park_timeout`, and shuts down cleanly on `Drop` (stop flag + `unpark` + `join`). Integrated as the first periodic job: `FSyncStrategy::Periodic(Duration)` opens a duplicate file descriptor each tick and calls `sync_all()`. The worker is restarted on segment rolls. Extracted `spawn_fsync_worker` helper to deduplicate the pattern across `new`, `from_dir`, and `roll_segment`. Updated `parse_fsync` to accept `every:Ns` syntax. Added 5 tests.

## #24 — Rust best practices cleanup

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/15

Applied idiomatic Rust improvements across the codebase:

1. `DB::new` returns `io::Result<DB>` instead of panicking on filesystem errors.
2. Reduced `unwrap()` in production paths — `main()` returns `io::Result<()>` and uses `?`; `roll_segment` maps `SystemTimeError` via `io::Error::other`.
3. Simplified `Record::read_next` — replaced verbose `match` with `let header = Record::read_header(file)?;`.
4. `Segment` derives `Clone` for cleaner usage in `from_dir`.
5. `ls_keys` returns `impl Iterator<Item = &String>` instead of leaking `hash_map::Keys`.
6. Removed redundant `parse::<String>()` calls in `settings.rs`.
7. Updated stale doc comments on `db.rs` methods and added docs to previously undocumented methods.

## #17 — Hint files for fast startup (DDIA Ch. 3, Bitcask paper)

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/14

Added hint files — sidecar `.hint` files written during compaction containing `(key_size, offset, tombstone, key)` tuples. On startup, `from_dir` loads the index from hint files when available (skipping value bytes), falling back to full record scan when no hint exists. Compaction writes one hint file per new segment and cleans up old hint files alongside old segments.

## #22 — Move Record free functions into impl block

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/13

Refactored `read_record`, `read_record_at`, and `append_record` from free functions in `record.rs` into methods on `Record`: `Record::read_next()`, `Record::read_record_at()`, `record.append()`. Updated all call sites in `db.rs`, `hash_index.rs`, and `tests/crc.rs`.

## #13 — Review sync strategy for write performance

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/12

`append_record` no longer calls `sync_all()` on every write. Durability is now controlled by a configurable `FSyncStrategy` enum (`Always`, `EveryN(n)`, `Never`) passed to `DB::new` / `DB::from_dir` and settable via `--fsync-interval` CLI flag. `Always` preserves the original behavior (default). Compaction unconditionally fsyncs before deleting old segments.

## #16 — Segment size limit + multi-segment reads (DDIA Ch. 3)

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/11

The DB uses a single segment that grows forever. DDIA describes how Bitcask rolls to a new segment file once the active one hits a size threshold, and compaction merges old segments. The work:

- Add a `max_segment_bytes` setting.
- When `append_record` would exceed the limit, close the current segment and open a new one.
- On read, if a key's offset refers to an older segment, open that file.
- Compaction merges all segments into one fresh segment.

This is the natural continuation of the existing segment infrastructure and teaches **log-structured storage lifecycle**.

## #15 — CRC checksums per record (DDIA Ch. 3)

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/10

The record format currently has no integrity check. Bitcask stores a CRC with every record so that corrupted bytes are detected on read rather than silently returning garbage. Add a CRC32 field to the record header (4 bytes, computed over key+value+tombstone), verify it in `read_record`, and return an error on mismatch. This teaches **data integrity at the storage layer** — a topic DDIA revisits in Chapters 3, 5, and 7.

## #21 — Fix clippy warnings

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/9

Fix all clippy warnings (`cargo clippy -- -D warnings`): redundant field name, identity op, needless borrows, needless `Ok(?)`  wrapper, missing `Default` impl, `SeekFrom::Current(0)` → `stream_position()`, missing `truncate` on `OpenOptions::create`, redundant `trim()` before `split_whitespace()`.

## #20 — Add agent config files and task backlog (#15–#19)

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/8

Add `AGENTS.md`, `CLAUDE.md`, `.github/copilot-instructions.md`, and `.github/hooks/post-edit.json` to the repo so that AI coding agents follow project conventions. Also add tasks #15–#19 to `TASKS.md` as the next batch of planned work (CRC checksums, segment size limits, hint files, SSTables, Bloom filters) and a "Closed Tasks" section.
