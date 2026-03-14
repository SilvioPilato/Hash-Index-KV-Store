# Hash Index KV Store

**⚠️ Experimental Project: This software is for experimental purposes only and is not intended for production use. Use at your own risk.**

This project implements a simple key-value store that communicates over TCP. It uses a hash index in memory to keep track of data stored across multiple segment files. On startup, if an existing database directory is provided, the server rebuilds the index by scanning all segment files so that previously stored data is available immediately.
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

You can interact with the server using a TCP client (e.g., `netcat` or `telnet`). The following commands are supported:

* **WRITE `<key> <value>`**: Stores the given value associated with the key. Values may contain spaces.
  * Example: `WRITE mykey myvalue`
* **READ `<key>`**: Retrieves the value associated with the key.
  * Example: `READ mykey`
* **DELETE `<key>`**: Deletes the key and its associated value.
  * Example: `DELETE mykey`
* **COMPACT**: Triggers background compaction of the database. Compaction rewrites only the latest values into fresh segment file(s), removing deleted keys and old overwrites, then deletes the old segment files. Writes block until compaction finishes. Concurrent compaction requests are rejected (returns `NOOP`).
  * Example: `COMPACT`
* **STATS**: Returns runtime counters as `key=value` lines:
  * `compacting` — whether compaction is currently running
  * `compaction_count` — number of completed compactions
  * `last_compact_start_ms` / `last_compact_end_ms` — timestamps of last compaction
  * `write_blocked_attempts` — writes that arrived during compaction
  * `write_blocked_total_ms` — total time writes spent waiting for the lock
  * `reads` / `writes` / `deletes` — operation counters
  * `active_connections` — current number of connected clients
  * Example: `STATS`

Each command should be sent on a new line, followed by an empty line to signify the end of the request.

### Example Interaction with `netcat`

1. Start the server: `cargo run -- /tmp/mydb`
2. In another terminal, connect with `netcat`: `nc localhost 6666`
3. Send commands (each followed by a blank line):

```text
WRITE name Alice

READ name

DELETE name

COMPACT

STATS
```
