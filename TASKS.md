# Open Tasks

## #13 — Review sync strategy for write performance

`append_record` currently calls `sync_all()` on every write, guaranteeing full on-disk durability but at the cost of write throughput (~5–20ms per fsync). Consider group commit, a configurable `sync` flag per write, or a periodic background sync (à la Redis `everysec`) once performance becomes a concern.

## #14 — Hardcoded port in integration tests

Integration tests bind to a hardcoded port (`6666`). If anything else is using that port, tests fail. A more robust approach would be to bind to port 0, have the server report the assigned port, and have tests read it back.

## #16 — Segment size limit + multi-segment reads (DDIA Ch. 3)

The DB uses a single segment that grows forever. DDIA describes how Bitcask rolls to a new segment file once the active one hits a size threshold, and compaction merges old segments. The work:

- Add a `max_segment_bytes` setting.
- When `append_record` would exceed the limit, close the current segment and open a new one.
- On read, if a key's offset refers to an older segment, open that file.
- Compaction merges all segments into one fresh segment.

This is the natural continuation of the existing segment infrastructure and teaches **log-structured storage lifecycle**.

## #17 — Hint files for fast startup (DDIA Ch. 3, Bitcask paper)

On startup, `HashIndex::from_file` does a full sequential scan of every record to rebuild the index. Bitcask solves this with **hint files** — a sidecar file containing only `(key, offset, tombstone)` tuples, written during compaction. On restart, loading the hint file is much faster since you skip all value bytes. This teaches the trade-off between **write amplification and recovery time**.

## #18 — Simple SSTable / sorted segments (DDIA Ch. 3)

Implement a **sorted string table** segment format alongside (or replacing) the current hash-indexed one. Writes go to an in-memory balanced tree (memtable); when it reaches a size threshold it's flushed as a sorted segment file. Reads check the memtable first, then segments newest-to-oldest. This is the foundation of LSM-Trees (LevelDB, RocksDB) and the second major storage engine architecture in DDIA Ch. 3. Start minimal — a single sorted segment flush + merge — and layer on a Bloom filter (#19) later.

## #19 — Bloom filter for key existence (DDIA Ch. 3)

Once there are multiple segments (from #16 or #18), checking every segment for a missing key is expensive. A per-segment **Bloom filter** lets you skip segments that definitely don't contain the key. Implementing one from scratch (bit array + k hash functions) is a good exercise in probabilistic data structures, directly referenced in DDIA's LSM-Tree discussion.

# Closed Tasks

<!-- Move completed tasks here to keep a reference of what was done. -->

## #15 — CRC checksums per record (DDIA Ch. 3)

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/10

The record format currently has no integrity check. Bitcask stores a CRC with every record so that corrupted bytes are detected on read rather than silently returning garbage. Add a CRC32 field to the record header (4 bytes, computed over key+value+tombstone), verify it in `read_record`, and return an error on mismatch. This teaches **data integrity at the storage layer** — a topic DDIA revisits in Chapters 3, 5, and 7.

## #21 — Fix clippy warnings

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/9

Fix all clippy warnings (`cargo clippy -- -D warnings`): redundant field name, identity op, needless borrows, needless `Ok(?)`  wrapper, missing `Default` impl, `SeekFrom::Current(0)` → `stream_position()`, missing `truncate` on `OpenOptions::create`, redundant `trim()` before `split_whitespace()`.

## #20 — Add agent config files and task backlog (#15–#19)

PR: https://github.com/SilvioPilato/Hash-Index-KV-Store/pull/8

Add `AGENTS.md`, `CLAUDE.md`, `.github/copilot-instructions.md`, and `.github/hooks/post-edit.json` to the repo so that AI coding agents follow project conventions. Also add tasks #15–#19 to `TASKS.md` as the next batch of planned work (CRC checksums, segment size limits, hint files, SSTables, Bloom filters) and a "Closed Tasks" section.
