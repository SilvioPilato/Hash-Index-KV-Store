# Hash Index KV Store

**⚠️ Experimental Project: This software is for experimental purposes only and is not intended for production use. Use at your own risk.**

This project implements a simple key-value store that communicates over TCP, built while reading *Designing Data-Intensive Applications*. It supports two storage engines selectable at startup:

- **KV (Bitcask)** — hash index in memory, append-only segment files, hint files for fast startup.
- **LSM** — in-memory memtable (BTreeMap) flushed to sorted string table (SSTable) segments, with sparse index for fast lookups and merge-sort compaction.

On startup, if an existing database directory is provided, the server rebuilds its state from segment files so that previously stored data is available immediately.
The purpose of the project is merely didactical, but if you want to tinker with it feel free to do it.

## Building

To build the project, use Cargo:

```sh
cargo build
```

## Running

To run the server, provide a directory path for the database as a command-line argument. The server creates a segment file inside that directory named `<segment_name>_<timestamp>.db` (e.g., `/tmp/mydb/segment_1700000000.db`):

```sh
cargo run -- <db_directory> [options]
```

### Options

| Flag                        | Description                           | Default         |
| --------------------------- | ------------------------------------- | --------------- |
| `-t`, `--tcp`               | TCP address to listen on              | `0.0.0.0:6666`  |
| `-n`, `--name`              | Segment file name prefix              | `segment`       |
| `-msb`, `--max-segments-bytes` | Max bytes per segment before rolling | `52428800` (50MB) |
| `-fsync`, `--fsync-interval` | Fsync strategy: `always`, `never`, `every:N` (every N writes), `every:Ns` (every N seconds) | `always` |
| `-e`, `--engine`             | Storage engine: `kv` (Bitcask) or `lsm` (LSM-tree) | `kv` |

### Examples

```sh
# Start with defaults (listens on 0.0.0.0:6666, segment prefix "segment")
cargo run -- /tmp/mydb

# Custom port and segment name
cargo run -- /tmp/mydb --tcp 127.0.0.1:7777 --name mydata
```

## Testing

To run all tests:

```sh
cargo test
```

The TCP server keeps its per-request debug logging off by default, so integration tests stay quiet. If you want the old connection and command logs while debugging, run the server with `KV_STORE_VERBOSE=1`.

## Commands

The server uses a **binary length-prefixed protocol** (not plain text). Each request is a frame: a 4-byte big-endian payload length, followed by a 1-byte op code, followed by op-specific fields. Responses use the same framing with a 1-byte status byte.

The easiest way to interact with the server is to build a client that uses the `bffp` module's `encode_frame`/`decode_response_frame` helpers, or use `netcat`/`telnet` with a tool that can send raw bytes.

### Supported commands

| Command | Op code | Description |
|---------|---------|-------------|
| `READ <key>` | 1 | Returns the value for `key`, or NOT_FOUND if absent |
| `WRITE <key> <value>` | 2 | Stores `value` under `key`. Values may contain spaces |
| `DELETE <key>` | 3 | Removes `key`. Returns NOT_FOUND if key does not exist |
| `COMPACT` | 4 | Triggers background compaction. Returns NOOP if already running |
| `STATS` | 5 | Returns runtime counters as `key=value` pairs |
| `LIST` | 6 | Returns all live keys as a list of strings |
| `EXISTS <key>` | 7 | Returns OK if `key` exists, NOT_FOUND if absent. On LSM, uses the Bloom filter for fast negative lookups |

### STATS fields

* `compacting` — whether compaction is currently running
* `compaction_count` — number of completed compactions
* `last_compact_start_ms` / `last_compact_end_ms` — timestamps of last compaction
* `write_blocked_attempts` — writes that arrived during compaction
* `write_blocked_total_ms` — total time writes spent waiting for the lock
* `reads` / `writes` / `deletes` — operation counters
* `active_connections` — current number of connected clients
