# In Progress

# Open Tasks

## #26 — Persist Bloom filters and sparse index to disk (DDIA Ch. 3)

Bloom filters and sparse indexes are currently rebuilt by scanning every SSTable file on startup. Serialize them to sidecar files (similar to hint files for Bitcask) so that LSM startup skips the full-file scan. Natural companion to the existing hint file infrastructure.

## #28 — mmap for SSTable reads (DDIA Ch. 3)

Memory-map SSTable files so lookups become pointer arithmetic instead of `read()` syscalls. Combined with the sparse index, this eliminates per-lookup I/O overhead. Good exercise in OS-level I/O and `unsafe` Rust, with platform-specific considerations (Windows vs. Unix).

## #29 — Block-based SSTable format with compression (DDIA Ch. 3)

Partition SSTables into fixed-size blocks (e.g., 4 KB) with per-block compression (e.g., hand-rolled LZ77 or simple run-length encoding). Index points to block offsets instead of individual records. Teaches data layout optimization and compression fundamentals.

## #31 — Connection timeouts and limits

Currently there is no read timeout and unbounded thread spawning per TCP connection. Add `SO_TIMEOUT` on sockets, a maximum connection limit, and graceful backpressure when the limit is reached. Addresses real operational concerns without changing the threading model.

## #32 — Async I/O with tokio

Replace the thread-per-connection TCP model with async handling using tokio. Enables higher concurrency with lower resource usage. A major Rust learning exercise and a stepping stone toward replication and distributed features.

## #33 — Single-leader replication (DDIA Ch. 5)

Add a `--role leader|follower` flag. The leader streams its write-ahead log to followers over TCP; followers replay it to maintain a replica. Teaches replication logs, consistency models, and failover — core DDIA Ch. 5 material. Depends on WAL (#25).

## #34 — Consistent hashing / partitioning (DDIA Ch. 6)

Shard the keyspace across multiple kv-store instances using consistent hashing or range-based partitioning. A coordinator node routes requests to the correct shard. Teaches DDIA Ch. 6 partitioning concepts: rebalancing, hot spots, and partition-aware routing.

## #36 — Per-operation latency histograms

Extend `Stats` to track per-operation latency distributions (p50/p95/p99). Implement a streaming quantile estimator (e.g., DDSketch or simple histogram buckets). Surface the results via the `STATS` command. Good exercise in streaming algorithms.

## #37 — Crash-recovery and fault-injection tests

Write tests that simulate crashes mid-write and mid-compaction (e.g., truncated files, partial records, missing hint files) and verify the engine recovers correctly. Validates the durability guarantees of both engines and exercises the CRC integrity checks.

## #40 — Engine info in `STATS` output

Extend the `STATS` command to include which engine is active, segment count, total data size on disk, and (for LSM) current memtable size. Makes the storage internals visible during interactive exploration.

## #43 — `DBINFO` command

Add a TCP command that dumps internal storage state: segment file listing, index size, bloom filter stats (estimated false positive rate), hint file presence, sparse index entry count. Lets you observe compaction shrinking segments and see the sparse index in action.

## #50 — `PREFIX` command (LSM only)

Add a `PREFIX <prefix>` TCP command that returns all key-value pairs whose keys start with the given string. LSM-only — implemented as a range scan `[prefix, prefix\xff]` on the sorted memtable and SSTables. The KV engine returns an error. Depends on #48 (`RANGE`) since it's a specialisation of range scan. Depends on #30 (binary protocol).

## #51 — `COUNT` command (LSM only)

Add a `COUNT <start> <end>` TCP command that returns the number of live keys in the inclusive range `[start, end]` without returning the values themselves. LSM-only. Shares the same merge-scan logic as `RANGE` but only emits a count. Depends on #48.

## #52 — `FIRST` and `LAST` commands (LSM only)

Add `FIRST` and `LAST` TCP commands that return the lexicographically smallest and largest live keys (with their values). LSM-only — trivially answered from the `BTreeMap` memtable and the first/last entries of the oldest/newest SSTables. The KV engine returns an error.

## #55 — `INCR` command

Add an `INCR <key>` TCP command that atomically increments an integer value stored at a key (creating it at 1 if absent). Returns the new value. Teaches read-modify-write atomicity — must be handled under the engine's write lock to avoid races. Supported by both engines.

## #56 — `TTL` command

Add a `TTL <key> <seconds>` TCP command that associates an expiry timestamp with a key. Expired keys are invisible to reads and cleaned up during compaction. Requires storing the expiry alongside the value in the record format (or as a separate metadata field). Good exercise in extending the on-disk format and compaction logic.

## #58 — `FLUSH` command (LSM only)

Add a `FLUSH` TCP command that forces an immediate memtable flush to a new SSTable, regardless of whether the flush threshold has been reached. LSM-only. Useful for testing, observability, and ensuring durability on demand. The KV engine returns an error (it has no memtable to flush).

## #59 — `SCAN` command

Add a `SCAN <cursor> <count>` TCP command for stateless paginated key iteration. The cursor is an opaque offset into the sorted keyspace; the server returns up to `count` keys starting at that offset plus the next cursor (or `0` when iteration is complete). Both engines support it — LSM iterates the sorted keyspace naturally; KV sorts the hash index keys at query time. Teaches stateless pagination and the tradeoffs of offset-based vs. hash-based cursors. Depends on #30 (binary protocol).

## #62 — Block-based segment format with compression for KV engine (DDIA Ch. 3)

Apply the block-based compression format (from #29) to the KV engine's append-only segments. Partition segments into fixed-size blocks with optional LZ77 compression per block. Update the hash index rebuild to work with blocks. Enables compression benefits for the Bitcask-style engine and demonstrates that block layouts are engine-agnostic. Depends on #29 (block format, LZ77 codec).

## #63 — Upgrade LZ77 to control-byte encoding (compression optimization, low priority)

Replace the varint-based LZ77 encoding (from #29) with control-byte encoding (Deflate/zlib style). Each control byte represents 8 operations (literals or match references), reducing metadata overhead and improving compression ratio by ~5%. Depends on #29. Low priority—varint is "good enough" for most workloads; this is a performance/space optimization for production use.

## #64 — Extend block header with integrity checks and versioning (low priority)

Extend the block-based SSTable format (from #29) with per-block integrity checks and format versioning. Add optional fields to the block header: (1) per-block CRC32 for early corruption detection, (2) block format version byte for forward/backward compatibility. This enables graceful format evolution without breaking existing SSTables. Depends on #29. Low priority—task #29 uses record-level CRC as the primary safety mechanism; this is an enhancement for production robustness.

# Closed Tasks

## #61 — Engine-internal concurrency: write buffering and fine-grained locking

Replace the single global `Arc<RwLock<Box<dyn StorageEngine>>>` with engine-internal locking so readers never wait for writers. `StorageEngine` trait methods change from `&mut self` to `&self` (interior mutability). KVEngine gets a write buffer (`RwLock<HashMap>`) with WAL for durability and batched flushes to disk. LsmEngine gets double-buffered memtables — an active `RwLock<Memtable>` and an immutable `RwLock<Option<Memtable>>` flushed to SSTable in a background thread. Both engines define explicit lock orderings to prevent deadlocks. `main.rs` drops the global lock entirely. Compaction blocking is out of scope (separate task). Design spec: `docs/superpowers/specs/2026-04-05-engine-concurrency-design.md`.

PR: <https://github.com/SilvioPilato/rustikv/pull/35>

## #60 — Extended kvbench scenarios (delete, overwrite, zipfian, mixed)

Added four new benchmark scenarios to `kvbench`: (1) DELETE phase — deletes a configurable fraction of keys and re-reads to measure tombstone overhead; (2) OVERWRITE phase — overwrites surviving keys N times to measure write amplification; (3) Zipfian read distribution — hot-key skewed reads via `--zipf <s>` to test Bloom filter effectiveness; (4) Mixed concurrent mode — writers and readers hit overlapping keys simultaneously for a configurable duration, exposing lock contention. Introduced `BenchConfig` struct to bundle parameters. New CLI flags: `--delete-ratio`, `--overwrite-rounds`, `--zipf`, `--mixed-duration`.

PR: <https://github.com/SilvioPilato/rustikv/pull/34>

## #27 — Leveled compaction (DDIA Ch. 3)

Implemented LevelDB-style leveled compaction as a `StorageStrategy`. Added `Level` struct with self-contained compaction triggers (L0: file count threshold, L1+: byte budget with 10x scaling per level). Cross-level merge via `compact_levels` merges source files with overlapping target files in one pass. Tombstones preserved on non-terminal levels, dropped on terminal. Leveled SSTable filenames encode the level (`{name}_L{n}_{timestamp}.sst`) for correct placement on restart. Wired into `main.rs` via `--storage-strategy leveled` with three new CLI flags (`-lnl`, `-ll0`, `-ll1`). 34 new tests in `tests/leveled.rs`.

PR: <https://github.com/SilvioPilato/rustikv/pull/33>

## #42 — Load generator / benchmark tool (`kvbench`)

Add a `cargo run --bin kvbench` binary that writes N random keys, reads them back, and prints throughput and latency stats. The key value: run it against `--engine kv` then `--engine lsm` to compare and make the write-amplification and read-amplification tradeoffs from DDIA Ch. 3 tangible.

PR: <https://github.com/SilvioPilato/rustikv/pull/32>

## #48 — `RANGE` command (LSM only)

Add a `RANGE <start> <end>` TCP command that returns all key-value pairs whose keys fall in the inclusive range `[start, end]`. Implement it only on the LSM engine — the KV (Bitcask) engine returns an error, making the hash-index limitation tangible. The LSM implementation merges results from the memtable (`BTreeMap::range`) and all SSTable iterators, applying tombstone suppression and returning the newest value per key in sorted order. Add `fn range(&self, start: &str, end: &str) -> Result<Vec<(String, String)>, io::Error>` to the `StorageEngine` trait. Depends on #30 (binary protocol) for clean multi-value response framing.

PR: <https://github.com/SilvioPilato/rustikv/pull/31>

## #53 — `MGET` command

Add a `MGET <key1> <key2> ...` TCP command that fetches multiple keys in a single round trip and returns their values (or null/missing markers for absent keys). Supported by both engines. Depends on #30 (binary protocol) for multi-value response framing.

PR: <https://github.com/SilvioPilato/rustikv/pull/30>

## #54 — `MSET` command

Add a `MSET <k1> <v1> <k2> <v2> ...` TCP command that writes multiple key-value pairs atomically in a single round trip. Supported by both engines. Reduces client-server overhead for bulk writes.

PR: <https://github.com/SilvioPilato/rustikv/pull/30>

## #57 — `PING` command

Add a `PING` TCP command that returns `PONG`. Trivial to implement — useful as a health check and connection keep-alive. Standard across Redis, Memcached, and most TCP servers.

PR: https://github.com/SilvioPilato/rustikv/pull/29

## #41 — CLI client (`rustikli`)

Add a `cargo run --bin rustikli` binary that connects to the server and provides a REPL-style interface for sending commands. Avoids the netcat "blank line after each command" friction and provides a nicer interactive experience.

PR: https://github.com/SilvioPilato/rustikv/pull/28

## #49 — `EXISTS` command

Add an `EXISTS <key>` TCP command that returns `1` if the key exists, `0` if not — without fetching the value. Both engines support it. On LSM, the bloom filter makes this especially efficient (fast negative lookups). Useful as a standalone command and as a building block for conditional operations.

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/27

## #39 — `LIST` command

There is currently no way to see what keys exist. Wire a `LIST` TCP command through the `StorageEngine` trait. `KVEngine` already has `ls_keys()` via `HashIndex`; `Memtable` has `entries()` for the LSM side. Return all keys to the client.

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/26

## #30 — Binary protocol with length-prefixed framing (DDIA Ch. 4)

Replace the text-based "line + blank line" TCP protocol with length-prefixed binary frames. Eliminates ambiguity around spaces in values, enables request pipelining, and is a good introduction to encoding formats and schema evolution (DDIA Ch. 4).

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/25

## #25 — WAL (Write-Ahead Log) for the LSM memtable (DDIA Ch. 3)

The LSM engine's memtable is currently volatile — a crash before flush loses all in-flight writes. Add a write-ahead log that persists every write before applying it to the memtable, and replays uncommitted entries on startup. This is a core LSM-tree concept directly from DDIA's discussion of log-structured storage.

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/24

## #35 — Automatic compaction trigger

Instead of manual `COMPACT` commands, trigger compaction automatically when dead-bytes / total-bytes exceeds a configurable threshold or when segment count exceeds a limit. The trigger runs in a background thread (matching the manual `COMPACT` pattern) so writes are never blocked. `segment_count()` added to the `StorageEngine` trait and implemented for both engines — KV tracks it via a field incremented on segment roll and reset on compaction; LSM returns `self.segments.len()`. Both conditions (ratio and segment count) are evaluated for every engine; natural zero-values disable the irrelevant condition per engine.

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/23

## #14 — Hardcoded port in integration tests

Integration tests now use OS-assigned port 0. Server writes actual bound address to a file that tests read back, with proper address conversion (0.0.0.0 → 127.0.0.1) for client connectivity. Thread-local storage and mutex poisoning recovery for reliable test execution.

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/22

## #47 — `LsmEngine::delete` always returns `Some(())`

Fixed `LsmEngine::delete` to return `Ok(None)` for nonexistent keys (matching KV engine behavior), not always `Ok(Some(()))`. Protocol consistency so TCP server says "Not found" for missing keys instead of always "OK". Updated corresponding test.

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/22

## #38 — `--help` usage message

Added proper help banner to `Settings::print_help()` that lists all CLI flags with descriptions and defaults. Running with no arguments or `-h/--help` displays usage instead of panicking.

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/22

## #46 — Concurrent `get()` races on shared file offset (Unix/Linux)

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/21

`KVEngine::get()` uses `try_clone()` on the active file for reads. On Unix/Linux, `dup()` shares the file offset across cloned descriptors, so concurrent readers (allowed by `RwLock::read()`) race on seek+read. Fixed by using `File::open()` for the active-segment read path instead of `try_clone()`, giving each reader an independent file descriptor. Added a concurrent-reads stress test (8 threads × 200 iterations × 100 keys) that exposed the race on Windows too.

## #45 — WRITE command loses whitespace fidelity

The `parse_message` function uses `split_whitespace` + `join(" ")` to reconstruct the value. This collapses consecutive spaces, tabs, and other whitespace into single spaces. For example, `WRITE key hello··world` (two spaces) stores `"hello world"` (one space). Fix by locating the value substring in the original input rather than splitting and re-joining.

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/20

## #44 — SSTableIter silently swallows all errors

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/19

`SSTableIter::next()` uses `.ok()` which converts **all** I/O and CRC errors into `None` (treated as EOF). This means corrupt records, CRC mismatches, and I/O failures are silently ignored. During `get()`, a corrupt record before the target key ends the scan early, returning "not found" even if the key exists. During `compact()`, corrupt records are silently dropped, causing **data loss**. The CRC32 integrity verification is effectively defeated for the entire LSM engine. The iterator should propagate errors instead of swallowing them.

## #19 — Bloom filter for key existence (DDIA Ch. 3)

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/18

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
