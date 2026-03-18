# In Progress

## #44 — SSTableIter silently swallows all errors

`SSTableIter::next()` uses `.ok()` which converts **all** I/O and CRC errors into `None` (treated as EOF). This means corrupt records, CRC mismatches, and I/O failures are silently ignored. During `get()`, a corrupt record before the target key ends the scan early, returning "not found" even if the key exists. During `compact()`, corrupt records are silently dropped, causing **data loss**. The CRC32 integrity verification is effectively defeated for the entire LSM engine. The iterator should propagate errors instead of swallowing them.

# Open Tasks

## #14 — Hardcoded port in integration tests

Integration tests currently use a hardcoded TCP port, which causes failures when tests run in parallel or on CI where the port may already be in use. Switch to port 0 (OS-assigned) so the OS picks an available port, and have the test read back the actual bound address. Quick hygiene fix.

## #25 — WAL (Write-Ahead Log) for the LSM memtable (DDIA Ch. 3)

The LSM engine's memtable is currently volatile — a crash before flush loses all in-flight writes. Add a write-ahead log that persists every write before applying it to the memtable, and replays uncommitted entries on startup. This is a core LSM-tree concept directly from DDIA's discussion of log-structured storage.

## #26 — Persist Bloom filters and sparse index to disk (DDIA Ch. 3)

Bloom filters and sparse indexes are currently rebuilt by scanning every SSTable file on startup. Serialize them to sidecar files (similar to hint files for Bitcask) so that LSM startup skips the full-file scan. Natural companion to the existing hint file infrastructure.

## #27 — Leveled compaction (DDIA Ch. 3)

Current LSM compaction merges all segments into a single SSTable. Real LSM-trees (LevelDB, RocksDB) use level-based compaction with size-tiered promotion between levels. Implementing this teaches write amplification tradeoffs and is the next natural step for the LSM engine.

## #28 — mmap for SSTable reads (DDIA Ch. 3)

Memory-map SSTable files so lookups become pointer arithmetic instead of `read()` syscalls. Combined with the sparse index, this eliminates per-lookup I/O overhead. Good exercise in OS-level I/O and `unsafe` Rust, with platform-specific considerations (Windows vs. Unix).

## #29 — Block-based SSTable format with compression (DDIA Ch. 3)

Partition SSTables into fixed-size blocks (e.g., 4 KB) with per-block compression (e.g., hand-rolled LZ77 or simple run-length encoding). Index points to block offsets instead of individual records. Teaches data layout optimization and compression fundamentals.

## #30 — Binary protocol with length-prefixed framing (DDIA Ch. 4)

Replace the text-based "line + blank line" TCP protocol with length-prefixed binary frames. Eliminates ambiguity around spaces in values, enables request pipelining, and is a good introduction to encoding formats and schema evolution (DDIA Ch. 4).

## #31 — Connection timeouts and limits

Currently there is no read timeout and unbounded thread spawning per TCP connection. Add `SO_TIMEOUT` on sockets, a maximum connection limit, and graceful backpressure when the limit is reached. Addresses real operational concerns without changing the threading model.

## #32 — Async I/O with tokio

Replace the thread-per-connection TCP model with async handling using tokio. Enables higher concurrency with lower resource usage. A major Rust learning exercise and a stepping stone toward replication and distributed features.

## #33 — Single-leader replication (DDIA Ch. 5)

Add a `--role leader|follower` flag. The leader streams its write-ahead log to followers over TCP; followers replay it to maintain a replica. Teaches replication logs, consistency models, and failover — core DDIA Ch. 5 material. Depends on WAL (#25).

## #34 — Consistent hashing / partitioning (DDIA Ch. 6)

Shard the keyspace across multiple kv-store instances using consistent hashing or range-based partitioning. A coordinator node routes requests to the correct shard. Teaches DDIA Ch. 6 partitioning concepts: rebalancing, hot spots, and partition-aware routing.

## #35 — Automatic compaction trigger

Instead of manual `COMPACT` commands, trigger compaction automatically when dead-bytes / total-bytes exceeds a configurable threshold or when segment count exceeds a limit. Uses the existing `BackgroundWorker` infrastructure. Small but impactful operational improvement.

## #36 — Per-operation latency histograms

Extend `Stats` to track per-operation latency distributions (p50/p95/p99). Implement a streaming quantile estimator (e.g., DDSketch or simple histogram buckets). Surface the results via the `STATS` command. Good exercise in streaming algorithms.

## #37 — Crash-recovery and fault-injection tests

Write tests that simulate crashes mid-write and mid-compaction (e.g., truncated files, partial records, missing hint files) and verify the engine recovers correctly. Validates the durability guarantees of both engines and exercises the CRC integrity checks.

## #38 — `--help` usage message

Running `cargo run` with no arguments currently panics. Add a proper `--help` / usage banner that lists all flags with descriptions and defaults, making the first-run experience clear.

## #39 — `LIST` command

There is currently no way to see what keys exist. Wire a `LIST` TCP command through the `StorageEngine` trait. `KVEngine` already has `ls_keys()` via `HashIndex`; `Memtable` has `entries()` for the LSM side. Return all keys to the client.

## #40 — Engine info in `STATS` output

Extend the `STATS` command to include which engine is active, segment count, total data size on disk, and (for LSM) current memtable size. Makes the storage internals visible during interactive exploration.

## #41 — CLI client (`kvcli`)

Add a `cargo run --bin kvcli` binary that connects to the server and provides a REPL-style interface for sending commands. Avoids the netcat "blank line after each command" friction and provides a nicer interactive experience.

## #42 — Load generator / benchmark tool (`kvbench`)

Add a `cargo run --bin kvbench` binary that writes N random keys, reads them back, and prints throughput and latency stats. The key value: run it against `--engine kv` then `--engine lsm` to compare and make the write-amplification and read-amplification tradeoffs from DDIA Ch. 3 tangible.

## #43 — `DBINFO` command

Add a TCP command that dumps internal storage state: segment file listing, index size, bloom filter stats (estimated false positive rate), hint file presence, sparse index entry count. Lets you observe compaction shrinking segments and see the sparse index in action.

## #45 — WRITE command loses whitespace fidelity

The `parse_message` function uses `split_whitespace` + `join(" ")` to reconstruct the value. This collapses consecutive spaces, tabs, and other whitespace into single spaces. For example, `WRITE key hello··world` (two spaces) stores `"hello world"` (one space). Fix by locating the value substring in the original input rather than splitting and re-joining.

## #46 — Concurrent `get()` races on shared file offset (Unix/Linux)

`KVEngine::get()` uses `try_clone()` on the active file for reads. On Unix/Linux, `dup()` shares the file offset across cloned descriptors, so concurrent readers (allowed by `RwLock::read()`) race on seek+read. This doesn't manifest on Windows (independent offsets via `DuplicateHandle`) but would corrupt reads on Linux. Fix by using `File::open()` for read paths instead of `try_clone()`.

## #47 — `LsmEngine::delete` always returns `Some(())`

`LsmEngine::delete` unconditionally returns `Ok(Some(()))` regardless of whether the key existed. The KV engine correctly returns `None` for missing keys, so the TCP server says `"OK"` vs `"Not found"` accordingly. The LSM engine always says `"OK"`. Not a data-correctness issue (tombstone for a nonexistent key is harmless), but a protocol-level inconsistency. Fix by checking the memtable and segments before returning.

# Closed Tasks

<!-- Move completed tasks here to keep a reference of what was done. -->

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
