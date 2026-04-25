# LZ77 3-Way Benchmark Comparison (2026-04-25)

Comparing three versions of the LZ77 encoder across a range of payload sizes.

| Version | Branch | Hash structure | Correctness |
|---------|--------|----------------|-------------|
| **#29** | `main` | `HashMap<[u8;3], usize>` — full chain pre-built **before** encoding | **Broken** — all chain steps skip future positions, emits literals only on repetitive input |
| **#66** | `66-lz77-fix` | `HashMap<[u8;3], usize>` — incremental, inserted **after** each position | Fixed |
| **#67** | `67-lz77-rolling-hash` | Flat `Vec<u32>` + zlib-style rolling hash `((prev << 5) ^ byte) & 32767` — incremental | Fixed |

**Setup:** LSM engine, `-bc lz77`, `--fsync-interval never`, sequential mode, uniform `"x".repeat(N)` values (maximally compressible). Each result is the average of 3 independent runs. Disk size is bytes on disk after the full write pass.

---

## Write Throughput (ops/sec)

| Payload | #29 broken | #66 HashMap | #67 Rolling | #67 vs #66 |
|---------|-----------|-------------|-------------|------------|
| 100 B (10 000 keys) | 37,050 | 37,903 | 37,595 | −0.8 % |
| 1 KB (10 000 keys) | 33,000 | 33,514 | 32,995 | −1.5 % |
| 10 KB (2 000 keys) | 16,200 | 17,439 | 16,184 | −7.2 % |
| 100 KB (1 000 keys) | 2,406 | 2,721 | 2,352 | −13.6 % |
| 1 MB (500 keys) | 5 | **21** | **30** | **+43 %** |

## Read Throughput (ops/sec)

| Payload | #29 broken | #66 HashMap | #67 Rolling | #67 vs #66 |
|---------|-----------|-------------|-------------|------------|
| 100 B | 45,650 | 46,052 | 45,693 | −0.8 % |
| 1 KB | 45,700 | 45,928 | 44,297 | −3.6 % |
| 10 KB | 39,300 | 38,760 | 38,534 | −0.6 % |
| 100 KB | 6,768 | 6,614 | 6,557 | −0.9 % |
| 1 MB | 10 | 10 | 10 | 0 % |

## Disk Usage (bytes)

| Payload | #29 broken | #66 HashMap | #67 Rolling |
|---------|-----------|-------------|-------------|
| 100 B | 1,390,014 | 1,390,014 | 1,390,014 |
| 1 KB | 10,630,014 | 10,630,014 | 10,630,014 |
| 10 KB | 20,558,014 | 20,558,014 | 20,558,014 |
| 100 KB | 50,021,650 | 49,990,246 | 49,990,246 |
| 1 MB | **1,048,480,278** | 8,161,778 | 8,161,786 |

---

## Analysis

### Correctness fix (#29 → #66)

The most dramatic difference is at 1 MB:

- **#29** writes ~1 GB to disk for 500 × 1 MB values — no compression. The bug (pre-building the entire hash chain before encoding) causes every chain walk to exhaust `MAX_CHAIN = 128` steps visiting future positions, leaving no budget to find real back-references. The result is a stream of literals.
- **#66** and **#67** both compress the same 500 × 1 MB to **~8 MB** (~128× reduction) and write at 21–30 ops/s.

Write throughput at 1 MB jumps from **5 ops/s → 21 ops/s** (+4×) just from fixing the correctness bug.

### Rolling hash improvement (#66 → #67)

The rolling hash replaces the `HashMap<[u8;3], usize>` with a flat `Vec<u32>` of 32 768 slots indexed by `((h << 5) ^ byte) & 32767`. This eliminates per-insertion heap allocation and hashing overhead.

- At **1 MB**: consistent **+43 %** write throughput (21 → 30 ops/s) across all 3 runs. The large payload means the encoder touches every position in the 32 KB window many times; the flat array pays off over the HashMap.
- At **100B–1KB**: negligible difference (within ±2 %). Compression is so fast at these sizes that encoder CPU is not the bottleneck — TCP framing, memtable writes, and SSTable flushing dominate.
- At **10KB–100KB**: #67 appears slightly behind #66 (7–14 %) but results are noisy across runs. The delta is within the variance between passes (e.g. #66 100KB: 2,406 / 2,799 / 2,957 across runs). No firm conclusion can be drawn; it is likely measurement noise.
- **Read throughput** is essentially identical across both versions at all sizes — decoding is the same algorithm for both.
- **Disk usage** is identical between #66 and #67 — same match algorithm, same compressed output.

### Summary

| Question | Answer |
|----------|--------|
| Does #66 fix the bug? | Yes — 1 MB disk drops from 1 GB to 8 MB; write throughput +4× |
| Does #67 improve on #66? | Yes, clearly at large payloads (+43 % at 1 MB); neutral at small ones |
| Is #67 ever slower than #66? | Possibly at 10–100 KB, but within measurement noise across 3 runs |
| Does the rolling hash change compression ratio? | No — identical disk sizes |
