# Redis Comparison Benchmark Setup

This document explains how to run `redis-compare` — comparing rustikv's TCP server against Redis.

## Quick Start

### 1. Start Redis (via Docker)

```bash
docker run -d -p 6379:6379 redis
```

Verify it's running:
```bash
redis-cli ping
# Response: PONG
```

### 2. Start rustikv

In a separate terminal:

```bash
cargo run --release -- /tmp/bench-db --engine lsm --fsync-interval never
```

Output should show:
```
Listening on 0.0.0.0:6666
```

### 3. Run the comparison

In a third terminal:

```bash
cargo run --release --bin redis-compare -- --count 10000 --value-size 100
```

## Output Format

The benchmark produces output in `kvbench` format for direct comparison:

```
# redis-compare: TCP server benchmark
# count: 10000
# value-size: 100

=== [rustikv] WRITE (10000 ops) ===
  Total:      25.123ms
  Throughput: 398,023 ops/sec
  Latency     min=1.234µs  mean=25.123µs  p99=123.456µs  max=234.567µs

=== [rustikv] READ (10000 ops) ===
  Total:      1.234ms
  Throughput: 8,104,283 ops/sec
  Latency     min=45.123ns  mean=123.456ns  p99=234.567ns  max=456.789ns

=== [redis] WRITE (10000 ops) ===
  Total:      15.234ms
  Throughput: 656,734 ops/sec
  Latency     min=0.123µs  mean=15.234µs  p99=67.234µs  max=123.456µs

=== [redis] READ (10000 ops) ===
  Total:      0.845ms
  Throughput: 11,834,024 ops/sec
  Latency     min=23.456ns  mean=84.567ns  p99=123.456ns  max=234.567ns
```

## CLI Options

```
--rustikv-host <addr>    TCP address of rustikv server  (default: 127.0.0.1:6666)
--redis-host <addr>      TCP address of Redis server    (default: 127.0.0.1:6379)
--count <n>              Number of keys to write/read   (default: 10000)
--value-size <bytes>     Bytes per value                (default: 100)
--engines <list>         Comma-separated: rustikv,redis (default: rustikv,redis)
```

### Examples

Run only Redis:
```bash
cargo run --release --bin redis-compare -- --engines redis --count 5000
```

Large payload (1 MB):
```bash
cargo run --release --bin redis-compare -- --value-size 1000000
```

Connect to non-local servers:
```bash
cargo run --release --bin redis-compare -- --rustikv-host 192.168.1.10:6666 --redis-host 192.168.1.11:6379
```

## Understanding the Results

### Throughput (ops/sec)

For **WRITE operations**, Redis will typically be 2-4× faster due to:
- In-memory operation (rustikv writes to disk)
- No fsync overhead
- Simpler key-value interface

For **READ operations**, the gap is smaller because:
- Both are fast on small values
- Memtable hits in rustikv are very fast
- Redis has no durability to verify

### Latency

- **WRITE latency**: microseconds (µs) for both
- **READ latency**: nanoseconds (ns) for both (cache hits, no I/O)

Watch for:
- p99 latency (tail latency) — rustikv may be higher due to fsync + compaction
- Max latency (worst case) — compaction can cause spikes

## Cleanup

Stop Redis:
```bash
docker stop <container-id>
```

Kill rustikv: `Ctrl+C` in the rustikv terminal

Remove benchmark data:
```bash
rm -rf /tmp/bench-db
```

## Interpreting Results

**rustikv vs Redis write throughput:**
- If rustikv is ~20-30% of Redis → **normal** (disk I/O vs in-memory)
- If rustikv is ~50%+ of Redis → **excellent** (unusually efficient TCP stack)
- If rustikv is <10% of Redis → **investigate** (network overhead, compaction interference)

**rustikv vs Redis read throughput:**
- If rustikv is within 50-100% of Redis → **good** (memtable hits dominate)
- If rustikv is >100% of Redis → **very good** (better caching than Redis?)
- If rustikv is <20% of Redis → **investigate** (cache misses, disk reads)

## Next: Payload Scaling

Repeat the benchmark with different payload sizes to see where rustikv's block-based storage and compression start to matter:

```bash
for size in 1000 10000 100000 1000000; do
  echo "=== $size byte values ==="
  cargo run --release --bin redis-compare -- --count 1000 --value-size $size
  echo
done
```
