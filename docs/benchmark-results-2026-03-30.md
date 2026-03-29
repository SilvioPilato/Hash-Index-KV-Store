# Benchmark Results — 2026-03-30

## Setup

- **Tool:** `kvbench` (`cargo run --bin kvbench`)
- **Keys:** 10,000
- **Value size:** 100 bytes
- **Scenarios:** KV/fsync=never, LSM/fsync=never, KV/fsync=always
- **Modes:** sequential, sequential with 30% miss ratio, concurrent (4 writers / 8 readers)

---

## Sequential — no misses

### WRITE

| | KV / fsync=never | LSM / fsync=never | KV / fsync=always |
| --- | --- | --- | --- |
| Throughput (ops/sec) | 26,691 | 30,400 | 467 |
| min | 25.8µs | 25.5µs | 1,887µs |
| mean | 37.4µs | 32.8µs | 2,140µs |
| p99 | 107.9µs | 89.8µs | 2,722µs |
| max | 1,249µs | 344.8µs | 9,233µs |
| total | 375ms | 329ms | 21.4s |

### READ

| | KV / fsync=never | LSM / fsync=never | KV / fsync=always |
| --- | --- | --- | --- |
| Throughput (ops/sec) | 15,286 | 38,799 | 15,555 |
| min | 48.8µs | 20.5µs | 48.1µs |
| mean | 65.3µs | 25.7µs | 64.2µs |
| p99 | 178.4µs | 75.4µs | 177.0µs |
| max | 5,306µs | 273.4µs | 4,993µs |
| total | 654ms | 258ms | 643ms |

---

## Sequential — 30% miss ratio

### WRITE

| | KV / fsync=never | LSM / fsync=never | KV / fsync=always |
| --- | --- | --- | --- |
| Throughput (ops/sec) | 28,749 | 30,295 | 469 |
| min | 26.1µs | 24.7µs | 1,902µs |
| mean | 34.7µs | 33.0µs | 2,133µs |
| p99 | 98.8µs | 98.2µs | 2,644µs |
| max | 269.3µs | 258.0µs | 7,846µs |
| total | 348ms | 330ms | 21.3s |

### READ (30% NOT_FOUND)

| | KV / fsync=never | LSM / fsync=never | KV / fsync=always |
| --- | --- | --- | --- |
| Throughput (ops/sec) | 18,588 | 38,350 | 20,002 |
| min | 19.8µs | 19.0µs | 19.4µs |
| mean | 53.7µs | 26.0µs | 49.9µs |
| p99 | 170.9µs | 74.7µs | 122.9µs |
| max | 5,060µs | 231.1µs | 6,095µs |
| total | 538ms | 261ms | 500ms |

---

## Concurrent — 4 writers / 8 readers

### WRITE

| | KV / fsync=never | LSM / fsync=never | KV / fsync=always |
| --- | --- | --- | --- |
| Throughput (ops/sec) | 2,428 | 61,057 | 405 |
| min | 25.5µs | 25.2µs | 1,942µs |
| mean | 1,647µs | 65.1µs | 9,824µs |
| p99 | 16,467µs | 182.8µs | 23,708µs |
| max | 37,861µs | 855.9µs | 64,075µs |
| total | 4.119s | 164ms | 24.7s |

### READ

| | KV / fsync=never | LSM / fsync=never | KV / fsync=always |
| --- | --- | --- | --- |
| Throughput (ops/sec) | 2,494 | 111,293 | 1,033 |
| min | 73.1µs | 20.7µs | 65.6µs |
| mean | 3,113µs | 69.1µs | 7,517µs |
| p99 | 9,717µs | 194.9µs | 16,661µs |
| max | 21,150µs | 640.4µs | 35,927µs |
| total | 4.010s | 90ms | 9.681s |

### AGGREGATE

| | KV / fsync=never | LSM / fsync=never | KV / fsync=always |
| --- | --- | --- | --- |
| Wall time | 4.119s | 164ms | 24.7s |
| Throughput (ops/sec) | 4,856 | 122,115 | 809 |

---

## Key Findings

| Finding | Evidence |
| --- | --- |
| **Writes are comparable sequentially** | KV 26.7K vs LSM 30.4K ops/sec — both append-only, difference is noise |
| **LSM reads 2.5× faster sequentially** | 38.8K vs 15.3K ops/sec — hash index O(1) vs SSTable scan (DDIA read amplification) |
| **LSM 25× faster under concurrent load** | 122K vs 4.9K aggregate ops/sec — KV write lock serialises all threads; LSM memtable absorbs concurrent writes without blocking readers |
| **KV p99 explodes under concurrency** | Write p99: 16.5ms KV vs 183µs LSM — tail latency reveals lock contention that mean latency hides |
| **fsync=always costs 65× write throughput** | 467 vs 26.7K sequential ops/sec; 405 vs 2.4K concurrent — one disk flush per write |
| **fsync=always hurts reads under concurrency too** | Read mean 7.5ms vs 3.1ms — readers queue behind writers waiting on disk |
| **Misses don't hurt LSM reads** | p99 unchanged (75µs hit vs 75µs miss) — Bloom filter eliminates disk access on NOT_FOUND |
| **Misses slightly help KV reads** | Mean drops from 65µs to 54µs — hash index returns nothing immediately, no segment I/O |
| **LSM max latency is tightly bounded** | Sequential max 273µs vs KV 5.3ms — no segment file seeking, memtable fits in cache |

---

## Payload scaling — sequential, fsync=never/always

Key counts scaled down for larger payloads to keep runtime reasonable (10KB: 2,000 keys; 100KB: 1,000; 1MB: 500).

### WRITE throughput (ops/sec)

| Value size | KV / fsync=never | LSM / fsync=never | KV / fsync=always |
| --- | --- | --- | --- |
| 100 B | 28,816 | 31,223 | 470 |
| 1 KB | 24,052 | 23,798 | 466 |
| 10 KB | 8,206 | 8,118 | 459 |
| 100 KB | 928 | 722 | 101 |
| 1 MB | 95 | 53 | 76 |

### WRITE latency

| Value size | KV / never mean | KV / never p99 | LSM / never mean | LSM / never p99 | KV / always mean | KV / always p99 |
| --- | --- | --- | --- | --- | --- | --- |
| 100 B | 34.6µs | 92.0µs | 32.0µs | 92.2µs | 2,129µs | 2,610µs |
| 1 KB | 41.5µs | 120.0µs | 42.0µs | 120.3µs | 2,148µs | 2,711µs |
| 10 KB | 121.8µs | 257.3µs | 123.1µs | 220.6µs | 2,178µs | 2,860µs |
| 100 KB | 1,077µs | 1,304µs | 1,386µs | 1,213µs | 9,922µs | 23,216µs |
| 1 MB | 10,523µs | 30,805µs | 19,001µs | 425,492µs | 13,097µs | 35,163µs |

### READ throughput (ops/sec)

| Value size | KV / fsync=never | LSM / fsync=never | KV / fsync=always |
| --- | --- | --- | --- |
| 100 B | 16,954 | 40,199 | 17,186 |
| 1 KB | 15,022 | 38,926 | 14,678 |
| 10 KB | 6,769 | 32,999 | 6,846 |
| 100 KB | 986 | 80 | 983 |
| 1 MB | 99 | 5 | 98 |

### READ latency

| Value size | KV / never mean | KV / never p99 | LSM / never mean | LSM / never p99 | KV / always mean | KV / always p99 |
| --- | --- | --- | --- | --- | --- | --- |
| 100 B | 58.9µs | 169.4µs | 24.8µs | 68.4µs | 58.1µs | 154.6µs |
| 1 KB | 66.5µs | 167.5µs | 25.6µs | 79.8µs | 68.1µs | 174.1µs |
| 10 KB | 147.7µs | 260.5µs | 30.3µs | 64.4µs | 146.0µs | 263.3µs |
| 100 KB | 1,014µs | 1,211µs | 12,455µs | 46,944µs | 1,017µs | 1,435µs |
| 1 MB | 10,086µs | 31,626µs | 213,953µs | 413,152µs | 10,176µs | 31,301µs |

### Key findings

| Finding | Evidence |
| --- | --- |
| **Writes converge as payload grows** | At 10KB both engines hit ~8K ops/sec — bottleneck shifts from index/protocol overhead to raw I/O, which is equal for both |
| **LSM reads collapse beyond 100KB** | 100KB: 80 ops/sec LSM vs 986 KV; 1MB: 5 vs 99 — large values fill segments fast, triggering frequent compaction flushes and merge-sorts mid-benchmark |
| **KV reads scale linearly with payload** | 59µs at 100B → 10ms at 1MB, proportional to bytes transferred over TCP; hash index seek cost is negligible |
| **LSM read p99 explodes at 1MB** | 413ms p99 vs 32ms for KV — compaction stalls cause extreme tail latency at large value sizes |
| **fsync=always write penalty shrinks with large payloads** | At 1MB: 95 (never) vs 76 (always) ops/sec — flush cost becomes negligible relative to time spent writing 1MB |
| **LSM advantage on reads vanishes above 10KB** | 10KB: LSM 33K vs KV 6.8K ops/sec; 100KB: LSM 80 vs KV 986 — the crossover point where compaction pressure overtakes the memtable read advantage |
