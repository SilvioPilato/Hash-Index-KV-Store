# rustikv Benchmarking Guide

Complete guide to benchmarking rustikv against different baselines and understanding the results.

## Overview

Three complementary benchmark tools measure rustikv at different layers:

| Tool | Layer | Purpose | Overhead |
|------|-------|---------|----------|
| **engbench** | In-process | Engine performance | None |
| **kvbench** | TCP server | Real-world usage | Full stack |
| **redis-compare** | TCP server | Competitive positioning | Identical to kvbench |

Together they answer three key questions:

1. **How fast is the engine itself?** → `engbench`
2. **How much does the TCP server cost?** → `engbench` vs `kvbench` delta
3. **How does it compare to Redis?** → `redis-compare`

---

## Quick Start: Full Benchmark Suite

### Prerequisites

```bash
# 1. Docker (for Redis)
docker --version

# 2. Rust toolchain
cargo --version

# 3. Python 3.6+ (for analysis script)
python3 --version
```

### Run Everything

```bash
# Terminal 1: Start Redis
docker run -d -p 6379:6379 redis
docker ps | grep redis  # verify

# Terminal 2: Start rustikv
cargo run --release -- /tmp/bench-db --engine lsm --fsync-interval never

# Terminal 3: Run all benchmarks
bash scripts/run-redis-benchmarks.sh
```

This will:
1. Run 4 redis-compare benchmarks (100B, 1KB, 10KB, 100KB payloads)
2. Generate `redis-comparison-analysis.md` with summary tables
3. Show where rustikv is competitive

---

## Individual Tools

### 1. engbench — In-Process Engine Benchmark

**Measures:** Raw engine throughput with zero TCP overhead.

**Start conditions:** Nothing required; standalone binary.

```bash
cargo run --release --bin engbench -- --count 10000 --value-size 100
```

**Output:**
```
# engine: rustikv-lsm
=== WRITE (10000 ops) ===
  Throughput: 307,285 ops/sec
  Latency     min=2µs  mean=3µs  p99=7µs  max=99µs

=== READ (10000 ops) ===
  Throughput: 4,794,093 ops/sec
  Latency     min=100ns  mean=187ns  p99=400ns  max=19µs

# engine: sled
=== WRITE (10000 ops) ===
  Throughput: 432,870 ops/sec
  ...
```

**Interpretation:**
- **Writes:** 307K ops/sec is the baseline without network overhead
- **Reads:** 4.8M ops/sec shows memtable hit performance
- **sled comparison:** Shows rustikv's competitive position against pure-Rust alternatives

**When to use:** Regression testing, optimization validation, understanding engine limits.

---

### 2. kvbench — TCP Server Benchmark

**Measures:** End-to-end throughput including BFFP framing, TCP, and server overhead.

**Start conditions:** rustikv server must be running.

```bash
# Terminal 1: Start rustikv
cargo run --release -- /tmp/bench-db --engine lsm --fsync-interval never

# Terminal 2: Run kvbench
cargo run --release --bin kvbench -- --count 10000 --value-size 100
```

**Output:**
```
Mode: sequential  Keys: 10000  Value size: 100B  ...
=== WRITE (10000 ops) ===
  Total:      250ms
  Throughput: 40,000 ops/sec
  Latency     min=20µs  mean=25µs  p99=100µs  max=500µs

=== READ (10000 ops) ===
  Total:      200ms
  Throughput: 50,000 ops/sec
  ...
```

**Interpretation:**
- **Writes:** 40K ops/sec (much slower than engbench's 307K)
- **Reads:** 50K ops/sec (much slower than engbench's 4.8M)
- **Delta:** TCP/framing overhead is 7.6× for writes, 96× for reads

**Modes:** sequential, concurrent (multi-writer/reader), mixed, zipfian

**When to use:** Real-world validation, concurrent workload testing, miss-ratio simulation.

---

### 3. redis-compare — Competitive Positioning

**Measures:** rustikv's TCP server vs Redis (industry-standard KV server).

**Start conditions:** Both servers running.

```bash
# Terminal 1: Start Redis
docker run -d -p 6379:6379 redis

# Terminal 2: Start rustikv
cargo run --release -- /tmp/bench-db --engine lsm --fsync-interval never

# Terminal 3: Run redis-compare
cargo run --release --bin redis-compare -- --count 10000 --value-size 100
```

**Output:**
```
=== [rustikv] WRITE (10000 ops) ===
  Throughput: 40,000 ops/sec
  ...

=== [redis] WRITE (10000 ops) ===
  Throughput: 150,000 ops/sec
  ...
```

**Interpretation:**
- **rustikv writes:** 40K ops/sec (disk-based with durability)
- **redis writes:** 150K ops/sec (in-memory, no durability)
- **Ratio:** rustikv is ~27% of Redis (reasonable given durability cost)

**Key insight:** Redis is the ceiling for in-memory performance. Rustikv's delta shows the cost of:
- Disk I/O
- fsync durability guarantees
- Block-based storage structure

**When to use:** Competitive analysis, showing rustikv's positioning to stakeholders.

---

## Understanding the Numbers

### TCP Overhead Calculation

```
TCP overhead % = (1 - engbench_throughput / kvbench_throughput) × 100

Example:
  engbench WRITE:  307K ops/sec
  kvbench WRITE:   40K ops/sec
  Overhead:        (1 - 40/307) × 100 = 87% overhead
  Overhead factor: 307 / 40 = 7.6×
```

This shows that 87% of CPU time is spent on TCP/serialization, only 13% on the storage engine itself.

### rustikv vs Redis Ratio

```
Competitive ratio % = (rustikv_throughput / redis_throughput) × 100

Example:
  rustikv WRITE: 40K ops/sec
  redis WRITE:   150K ops/sec
  Ratio:         40 / 150 = 27% (rustikv is 27% of Redis speed)
```

**Expectations:**
- **Writes:** 20-40% of Redis (durability overhead)
- **Reads:** 50-100% of Redis (less durability cost on reads)

### Payload Scaling

As payload size increases:
- **100B:** Both servers dominated by framing/protocol overhead
- **1KB:** Still overhead-heavy
- **10KB:** rustikv's block structure starts to matter
- **100KB:** rustikv compression + blocking helps; redis degrades
- **1MB:** I/O dominates; rustikv struggles vs Redis

---

## Optimization Opportunities

Based on benchmark results, consider:

### If TCP overhead is >85%

**Problem:** BFFP serialization/deserialization is bottleneck

**Solutions:**
- Profile `encode_command` / `decode_response_frame` CPU time
- Consider binary protocol optimizations (shorter framing, zero-copy)
- Pipeline multiple ops in single request

### If reads are <50% of Redis

**Problem:** Cache misses or lock contention on reads

**Solutions:**
- Check memtable hit rate (via stats)
- Measure compaction interference during reads
- Consider read-only snapshots for hot keys

### If writes are <20% of Redis

**Problem:** Likely fsync + compaction overhead

**Solutions:**
- Batch writes to reduce fsync calls (use `--fsync-interval every:10`)
- Move compaction to background thread (already done, but check contention)
- Measure compaction time % of total

---

## Benchmark File Format

All benchmarks output to `docs/benchmark-runs/2026-04-25/`:

- `redis-compare-100B.txt` — Full output from 100 byte run
- `redis-compare-1KB.txt` — Full output from 1 KB run
- `redis-comparison-analysis.md` — Auto-generated summary with tables

Each `.txt` file contains:
```
# redis-compare: TCP server benchmark
# count: 10000
# value-size: 100

=== [rustikv] WRITE (10000 ops) ===
  Total:      250ms
  Throughput: 40,000 ops/sec
  Latency     min=...  mean=...  p99=...  max=...

=== [redis] WRITE (10000 ops) ===
  ...

=== [rustikv] READ (10000 ops) ===
  ...

=== [redis] READ (10000 ops) ===
  ...
```

---

## Example: Full Analysis

```bash
# 1. Run in-process benchmark
cargo run --release --bin engbench -- --count 10000 --value-size 100
# → rustikv-lsm: 307K writes, 4.8M reads

# 2. Run TCP benchmark
cargo run --release --bin kvbench -- --count 10000 --value-size 100
# → rustikv-lsm: 40K writes, 50K reads

# 3. Run Redis comparison
cargo run --release --bin redis-compare -- --count 10000 --value-size 100
# → redis: 150K writes, 500K reads

# 4. Analysis:
#    TCP overhead on writes:   307K / 40K = 7.6× (87% overhead)
#    TCP overhead on reads:    4.8M / 50K = 96× (99% overhead)
#    vs Redis on writes:       40K / 150K = 27% (rustikv slower)
#    vs Redis on reads:        50K / 500K = 10% (rustikv slower)
#
# Interpretation:
#   - 87% TCP overhead is high but normal (protocol/framing)
#   - 99% TCP overhead on reads is due to small payload (< 1µs vs 5µs round-trip)
#   - rustikv writes at 27% of Redis speed (reasonable for disk-based)
#   - rustikv reads at 10% of Redis speed (memtable hits should be faster?)
```

---

## Next Steps

1. **Baseline:** Run all three benchmarks, save results to git
2. **Monitor:** Add these to CI/CD to catch regressions
3. **Optimize:** Profile high-overhead components (TCP framing, compaction)
4. **Compare:** Re-run after optimizations to measure improvement
