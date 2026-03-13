# Open Tasks

## #13 — `flush()` vs `sync_all()` for durability

`append_record` calls `flush()` which only pushes data to the OS page cache. For actual on-disk durability, `sync_all()` (or `sync_data()`) is needed. This is a trade-off between write performance and crash safety.

## #14 — Hardcoded port in integration tests

Integration tests bind to a hardcoded port (`6666`). If anything else is using that port, tests fail. A more robust approach would be to bind to port 0, have the server report the assigned port, and have tests read it back.
