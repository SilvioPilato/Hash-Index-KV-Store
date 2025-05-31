# Hash Index KV Store

**⚠️ Experimental Project: This software is for experimental purposes only and is not intended for production use. Use at your own risk.**

This project implements a simple key-value store that communicates over TCP. It uses a hash index in memory to keep track of data stored in a file.
The purpose of the project is merely didactical, but if you want to tinker with it feel free to do it.

## Building

To build the project, use Cargo:

```sh
cargo build
```

## Running

To run the server, provide a path for the database file as a command-line argument:

```sh
cargo run -- <path_for_db_file>
```

For example:

```sh
cargo run -- /tmp/my_database
```

The server will start listening on `0.0.0.0:6666`.

## Commands

You can interact with the server using a TCP client (e.g., `netcat` or `telnet`). The following commands are supported:

* **WRITE `<key> <value>`**: Stores the given value associated with the key.
  * Example: `WRITE mykey myvalue`
* **READ `<key>`**: Retrieves the value associated with the key.
  * Example: `READ mykey`
* **DELETE `<key>`**: Deletes the key and its associated value.
  * Example: `DELETE mykey`

Each command should be sent on a new line, followed by an empty line to signify the end of the request.

### Example Interaction with `netcat`

1. Start the server: `cargo run -- /tmp/mydb`
2. In another terminal, connect with `netcat`: `nc localhost 6666`
3. Send commands:
```
    WRITE name Alice

    READ name

    DELETE name

```
