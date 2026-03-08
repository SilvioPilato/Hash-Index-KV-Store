# Hash Index KV Store

**⚠️ Experimental Project: This software is for experimental purposes only and is not intended for production use. Use at your own risk.**

This project implements a simple key-value store that communicates over TCP. It uses a hash index in memory to keep track of data stored in a file. On startup, if an existing database file is provided, the server rebuilds the index by scanning the file so that previously stored data is available immediately.
The purpose of the project is merely didactical, but if you want to tinker with it feel free to do it.

## Building

To build the project, use Cargo:

```sh
cargo build
```

## Running

To run the server, provide a base path for the database file as a command-line argument. The server appends a timestamp and `.db` extension to generate the actual filename (e.g., `/tmp/mydb_1700000000.db`):

```sh
cargo run -- <path_for_db_file>
```

For example:

```sh
cargo run -- /tmp/my_database
```

The server will start listening on `0.0.0.0:6666`.

## Testing

To run all tests (7 unit + 3 integration):

```sh
cargo test
```

## Commands

You can interact with the server using a TCP client (e.g., `netcat` or `telnet`). The following commands are supported:

* **WRITE `<key> <value>`**: Stores the given value associated with the key.
  * Example: `WRITE mykey myvalue`
* **READ `<key>`**: Retrieves the value associated with the key.
  * Example: `READ mykey`
* **DELETE `<key>`**: Deletes the key and its associated value.
  * Example: `DELETE mykey`
* **COMPACT**: Triggers background compaction of the database file. Compaction rewrites only the latest values into a new file, removing deleted keys and old overwrites. Writes block until compaction finishes. Concurrent compaction requests are rejected.
  * Example: `COMPACT`
* **STATS**: Returns runtime counters as `key=value` lines. Includes read/write/delete counts, active connections, compaction state, and write-blocking metrics.
  * Example: `STATS`

Each command should be sent on a new line, followed by an empty line to signify the end of the request.

### Example Interaction with `netcat`

1. Start the server: `cargo run -- /tmp/mydb`
2. In another terminal, connect with `netcat`: `nc localhost 6666`
3. Send commands:
```
    WRITE name Alice

    READ name

    DELETE name

    COMPACT

    STATS

```
