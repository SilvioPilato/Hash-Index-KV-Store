# Open Tasks

## #13 — Review sync strategy for write performance

`append_record` currently calls `sync_all()` on every write, guaranteeing full on-disk durability but at the cost of write throughput (~5–20ms per fsync). Consider group commit, a configurable `sync` flag per write, or a periodic background sync (à la Redis `everysec`) once performance becomes a concern.

## #14 — Hardcoded port in integration tests

Integration tests bind to a hardcoded port (`6666`). If anything else is using that port, tests fail. A more robust approach would be to bind to port 0, have the server report the assigned port, and have tests read it back.
