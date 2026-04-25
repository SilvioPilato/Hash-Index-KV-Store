# LZ77 Incremental Chain Building — Benchmark Results (2026-04-24)

**Task #66:** Fix LZ77 compression quality on low-entropy input via incremental hash chain building.

**Codec:** `src/lz77.rs` — 32 KB sliding window, 258-byte lookahead, `MAX_CHAIN = 128`, varint-encoded tokens.

**Key fix:** The hash table is now built incrementally during encoding (position `pos` is inserted *after* it is processed). Before #66, the full chain was pre-built across the entire input, causing the encoder to exhaust its `MAX_CHAIN` budget skipping future-position candidates before finding any real back-reference. Result: uniform input produced only literals. After #66: all 128 chain steps evaluate real match candidates.

---

## 1. Compression Ratio

Measured with `lz77bench` (release build). All roundtrips verified.

| Input type | Original | Compressed | Ratio | Time (ms) | Throughput |
|---|---|---|---|---|---|
| Uniform 10 KB (`'a'` × 10 000) | 10 000 | 158 | **0.016** | < 1 | 18.9 MB/s |
| Repetitive text 6 KB (`"hello world "` × 500) | 6 000 | 119 | **0.020** | < 1 | 20.2 MB/s |
| Mixed (unique prefix + `"pattern"` × 1000 + unique suffix) | 7 256 | 637 | **0.088** | < 1 | 20.0 MB/s |
| Natural text ~600 KB (Lorem ipsum × 1 500) | 670 500 | 13 715 | **0.020** | 23 | 27.8 MB/s |
| Binary alternating 100 KB (`0xFF/0x00`) | 102 400 | 1 592 | **0.016** | 4 | 19.7 MB/s |
| Random 100 KB (LCG pseudo-random) | 102 400 | 204 540 | **2.00** | 5 | 17.9 MB/s |

**Observations:**
- Highly structured inputs (uniform bytes, alternating binary, repetitive text, natural prose) all compress to **1.6–2.0 %** of the original size — 50–64× reduction.
- Mixed input with unique boundaries compresses to **8.8 %** (11× reduction); the unique prefix/suffix bytes emit as literals.
- Random (high-entropy) input **expands** to ~2×, as expected — no back-references exist.

---

## 2. Performance (Large Inputs)

| Input type | Original | Compressed | Ratio | Time (ms) | Throughput |
|---|---|---|---|---|---|
| 1 MB uniform bytes (`'x'` × 1 048 576) | 1 048 576 | 16 261 | 0.016 | 49 | **20.3 MB/s** |
| 10 MB repetitive text (`"the quick brown fox "` × …) | 10 000 000 | 155 080 | 0.016 | 477 | **20.0 MB/s** |
| 1 MB random data (LCG) | 1 048 576 | 2 093 986 | 2.00 | 76 | **13.1 MB/s** |

**Observations:**
- 10 MB repetitive text encodes in **477 ms** — well under the 3-second target.
- Throughput is stable at **20 MB/s** for compressible data; drops to **13 MB/s** for random data (more literal tokens, less match reuse).
- All three roundtrips pass.

---

## 3. End-to-End: LSM Engine — `lz77` vs `none`

Setup: `rustikv --engine lsm --fsync never --max-segments-bytes 2097152` (2 MB memtable threshold to force multiple SSTable flushes). 20 000 keys × 1 024 B values = **20 MB total data**. `kvbench` sequential mode; values are `"x".repeat(1024)` (uniform, highly compressible).

### Throughput

| Phase | `lz77` | `none` | Delta |
|---|---|---|---|
| WRITE | 22 440 ops/sec | 35 339 ops/sec | **−36 %** |
| READ | 3 166 ops/sec | 4 378 ops/sec | **−28 %** |
| RANGE (window=100) | 168 ops/sec | 224 ops/sec | **−25 %** |

### Latency

| Phase | `lz77` mean | `none` mean | `lz77` p99 | `none` p99 |
|---|---|---|---|---|
| WRITE | 44.5 µs | 28.3 µs | 83 µs | 90 µs |
| READ | 315 µs | 228 µs | 702 µs | 521 µs |
| RANGE | 5.95 ms | 4.46 ms | 12.5 ms | 9.4 ms |

### On-disk Size

| Arm | Disk bytes | Ratio vs raw data (20 MB) |
|---|---|---|
| `lz77` | 2 929 458 (~2.9 MB) | **14.3 %** — 7.0× compression |
| `none` | 21 314 365 (~21.3 MB) | 104 % — 1.0× (includes overhead) |

### Observations

- **Disk savings are dramatic:** lz77 stores 20 MB of uniform data in 2.9 MB on disk — a **7× reduction** (the extra overhead beyond the codec's 64× comes from record headers, WAL bytes, and sparse index data that are not block-compressed).
- **CPU cost is real:** compressing and decompressing 4 KB blocks on every flush/read adds latency. Write throughput drops 36 %, reads drop 28 %. For high-entropy (random) data, lz77 would add overhead *without* space savings — users should use `--block-compression none` for such workloads.
- **The performance gap narrows at larger memtables:** at the default 50 MB threshold, both arms are indistinguishable (flushes are rare and data stays in the memtable for most of the run). The overhead is only visible under forced-flush conditions (small `-msb`).

---

## 4. Summary

| Metric | Before #66 | After #66 |
|---|---|---|
| Uniform input compression | ❌ No compression (all literals) | ✅ 64× reduction |
| Repetitive text compression | ❌ No compression | ✅ 50× reduction |
| 10 MB encode time | ❌ Unbounded (> 3s) | ✅ 477 ms (20 MB/s) |
| Random data encode | ✅ Correctly expands | ✅ Correctly expands |
| E2E disk reduction (kvbench) | — | ✅ 7× vs `--block-compression none` |
| Write throughput cost | — | −36 % (expected, compression CPU) |
| Read throughput cost | — | −28 % (expected, decompression CPU) |

**The incremental chain fix fully restores compression quality for structured data** while keeping encode time bounded at ~20 MB/s. The only tradeoff is CPU overhead on the LSM read/write path, which is the expected cost of compression.
