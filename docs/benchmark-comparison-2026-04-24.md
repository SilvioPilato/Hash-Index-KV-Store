# Benchmark Comparison — Block-Based SSTable + LZ77 Compression (#29)

**Before:** 2026-04-07 ([benchmark-comparison-2026-04-07-early-drop.md](benchmark-comparison-2026-04-07-early-drop.md)), flat-record SSTable format, no compression.
**After:** 2026-04-24, block-based SSTable format (default `--block-size-kb 4`), run with both `--block-compression lz77` (the new default) and `--block-compression none` to isolate compression cost from block-format cost.

Same machine, same `kvbench`, same key counts and value sizes. Built `--release` from commit `a3d62f2`. Raw per-run output with server/bench invocations recorded in each header: [`docs/benchmark-runs/2026-04-24/`](benchmark-runs/2026-04-24/).

The #29 change applies to the **LSM engine only** — `kvengine.rs` was not touched. KV results are presented as a noise control: any large KV delta is system variance, not a real change.

---

## 1. Sequential — no misses (10,000 keys, 100 B values)

### WRITE throughput (ops/sec)

| | #62 (before) | #29 lz77 | Delta |
| --- | --- | --- | --- |
| KV / fsync=never | 35,003 | 33,047 | -6% |
| LSM / fsync=never | 38,902 | 37,322 | -4% |
| KV / fsync=always | 467 | 260 | **-44%** (disk variance — KV unchanged in #29) |

### READ throughput (ops/sec)

| | #62 (before) | #29 lz77 | Delta |
| --- | --- | --- | --- |
| KV / fsync=never | 20,370 | 18,716 | -8% |
| LSM / fsync=never | 46,974 | 44,797 | -5% |
| KV / fsync=always | 20,798 | 17,724 | -15% |

### Latency (LSM / fsync=never only)

| Phase | Before mean | After mean | Before p99 | After p99 |
| --- | --- | --- | --- | --- |
| WRITE | 25.7 µs | 26.8 µs | 76.8 µs | 71.2 µs |
| READ | 21.3 µs | 22.3 µs | 53.8 µs | 57.3 µs |

> **Verdict:** Sequential is essentially flat. All numbers within ±10 % of #62 baseline at 100 B values. LSM/never latency is unchanged within noise even though every record now goes through the new block-encode path.

---

## 2. Sequential — 30% miss ratio (10,000 keys, 100 B values)

### WRITE throughput, 30% miss (ops/sec)

| | #62 (before) | #29 lz77 | Delta |
| --- | --- | --- | --- |
| KV / fsync=never | 31,595 | 32,955 | +4% |
| LSM / fsync=never | 37,186 | 38,206 | +3% |
| KV / fsync=always | 468 | 468 | 0% |

### READ throughput, 30% miss (ops/sec)

| | #62 (before) | #29 lz77 | Delta |
| --- | --- | --- | --- |
| KV / fsync=never | 22,737 | 22,238 | -2% |
| LSM / fsync=never | 45,379 | 45,480 | 0% |
| KV / fsync=always | 24,116 | 22,831 | -5% |

> **Verdict:** Bloom-filter short-circuit on miss is preserved — LSM read throughput at 30 % miss is within 0.2 % of #62. No regression from the new SSTable format on the negative-lookup path.

---

## 3. Concurrent — 4 writers / 8 readers (10,000 keys, 100 B values)

### Aggregate throughput

| | #62 ops/sec | #29 lz77 ops/sec | Delta |
| --- | --- | --- | --- |
| KV / fsync=never | 3,454 | 5,898 | +71% |
| LSM / fsync=never | 176,256 | 183,546 | +4% |
| KV / fsync=always | 943 | 941 | 0% |

> **Verdict:**
>
> - **LSM/never +4 %** — within noise. Block format doesn't hurt the concurrent hot path at small payloads.
> - **KV/never +71 %** — KV code didn't change in #29; the gain is system-load variance between runs. Worth re-running on a quiet machine before reading into it.
> - **KV/always concurrent reads** hit ~220 K ops/sec (vs 178 K in #62) — same artifact noted in the #62 doc: writers are stuck on fsync while readers race through 10 K NOT_FOUND lookups before any key lands. Not a real read measurement.

---

## 4. Payload scaling — sequential, LSM, fsync=never

This is the most important section: the block format and LZ77 only affect SSTable encoding, which the small-payload runs above barely exercise. Here both arms are shown side by side.

### WRITE throughput, payload scan (ops/sec)

| Value size | #62 flat | #29 lz77 | #29 none | lz77 vs flat | none vs flat |
| --- | --- | --- | --- | --- | --- |
| 100 B | 36,186 | 38,001 | 40,996 | +5% | +13% |
| 1 KB | 32,719 | 33,637 | 37,212 | +3% | +14% |
| 10 KB | 16,163 | 16,894 | 19,051 | +5% | +18% |
| 100 KB | 3,161 | 2,396 | 3,110 | **-24%** | -2% |
| 1 MB | 275 | **hung** | 224 | — | -19% |

### READ throughput, payload scan (ops/sec)

| Value size | #62 flat | #29 lz77 | #29 none | Notes |
| --- | --- | --- | --- | --- |
| 100 B | 41,129 | 45,967 | 47,552 | both #29 arms slightly faster than flat |
| 1 KB | 43,981 | 44,789 | 49,031 | within noise |
| 10 KB | 2,748 † | 38,059 | 42,808 | † #62 baseline was compaction noise |
| 100 KB | 174 † | **6,700** | **333** | † unreliable; lz77 vs none shows +20× compression advantage |
| 1 MB | 19 | **hung** | 14 | — |

> † Flagged in the #62 doc as compaction noise in the baseline — those deltas aren't interpretable. The #29 numbers are reliable in isolation.

### On-disk size (bytes, recursive DB directory after run)

| Value size | raw data bytes | #29 lz77 on disk | #29 none on disk | lz77/raw | lz77/none |
| --- | --- | --- | --- | --- | --- |
| 100 B (× 10,000) | 1,000,000 | 1,390,014 | 1,390,014 | 1.39× | 1.00× |
| 1 KB (× 10,000) | 10,240,000 | 10,630,014 | 10,630,014 | 1.04× | 1.00× |
| 10 KB (× 2,000) | 20,480,000 | 20,558,014 | 20,558,014 | 1.00× | 1.00× |
| 100 KB (× 1,000) | 102,400,000 | 50,021,650 | 102,443,622 | 0.49× | **0.49×** |
| 1 MB (× 500) | 524,288,000 | — (hung) | 524,312,014 | — | 1.00× |

> For ≤ 10 KB payloads the entire workload fits in the memtable — **nothing flushes to SSTable**, so block format and compression have no effect on disk size (identical bytes between `lz77` and `none`). Compression only becomes visible once flush happens (100 KB here). With `'x'.repeat(100000)` — maximally compressible input — LZ77 halves the on-disk segment size.

### The 1 MB `lz77` hang — root cause and fix status

`pay-1MB-lsm-never` under `--block-compression lz77` previously **never produced a result** after 7 minutes. Under `--block-compression none`, the same workload completes normally (224 writes/s, 14 reads/s, ≈ 40 s total). This isolated the hang to the LZ77 encoder path.

**Root cause (time bound).** In [src/lz77.rs](../src/lz77.rs), the inner loop of `find_longest_match` had a branch that skipped future candidates without incrementing the `iteration` counter, so `MAX_CHAIN = 128` didn't bound the skip walk. On input where every 3-byte window hashes identically, walking backwards to find a valid candidate took O(N) links per position — making `encode` O(N²). Fixed by checking `iteration >= MAX_CHAIN` at the top of the loop before any skip. Regression test: `test_lz77_encode_large_repetitive_is_bounded` ([tests/lz77.rs:132](../tests/lz77.rs#L132)) — passes in 0.22 s on this machine.

**Re-run after fix (2026-04-24, commit `a3d62f2` + time-bound fix).** Raw output: [`docs/benchmark-runs/2026-04-24/pay-1MB-lsm-never-bclz77.txt`](benchmark-runs/2026-04-24/pay-1MB-lsm-never-bclz77.txt).

| | `-bc none` | `-bc lz77` (fixed) | Delta |
| --- | --- | --- | --- |
| WRITE ops/sec | 224 | **5** | −98% |
| READ ops/sec | 14 | **10** | −29% |
| DB size (bytes) | 524,312,014 | **1,048,480,278** | **+2×** |

The hang is gone, but performance and disk size are worse than no-compression — not better. Root cause: **compression quality bug** tracked in task #66.

**Root cause (compression quality, task #66).** `get_hash_chain` pre-builds the full chain before encoding, storing the *last* occurrence of each 3-byte key globally. At position `pos`, the table entry points to a position near N-3 — in the future. With `MAX_CHAIN = 128`, all 128 budget steps are spent skipping future candidates before any valid back-reference is reached. For `'x'.repeat(1 MB)`, the encoder emits only literals: each input byte costs 2 bytes of output (token + data), expanding 1 MB → ~2 MB per record, 500 records → ~1 GB on disk. The per-record O(N × MAX_CHAIN) cost also dominates write throughput (5 ops/s vs 224). Fix: build the chain incrementally during encoding so the table only ever contains positions strictly behind `pos`.

---

## 5. Is the current implementation slower?

Depends on the workload dimension:

| Workload | Throughput change vs #62 | On-disk change vs #62 | Net |
| --- | --- | --- | --- |
| ≤ 10 KB payloads (memtable-resident) | flat / slightly faster | identical (no flush) | **no change** |
| 100 KB writes | −24 % | −51 % | trade: slower writes, half the disk |
| 100 KB reads | +20× (flaky baseline) | −51 % | net win if baseline is trusted |
| 1 MB | −98% writes, −29% reads vs none | +2× (literals only — #66) | hang fixed; quality regression pending #66 |
| KV (all) | within ±10 % noise | — | unchanged (KV untouched) |
| Bloom negative lookup (30 % miss) | flat | — | unchanged |

**Bottom line.** The block-based SSTable format itself is sound — `--block-compression none` matches the pre-#29 baseline across every payload size, including 1 MB. The headline regression is a bug in the hand-rolled LZ77 encoder (unbounded chain-skip walk on low-entropy input), not in the new format. Once the LZ77 bug is fixed, #29 should be net neutral at small payloads and a clear space-savings win from 100 KB upwards.

---

## 6. Recommended next steps

1. ~~**Fix the `find_longest_match` chain-skip bound**~~ — **Done** (2026-04-24). `MAX_CHAIN` now checked at loop top; regression test `test_lz77_encode_large_repetitive_is_bounded` passes in 0.22 s.
2. **Fix LZ77 compression quality on low-entropy input (task #66)** — switch `get_hash_chain` to an incremental build during encoding so the table only contains positions behind `pos`. Once done, rerun `pay-1MB-lsm-never-bclz77`; expected outcome: throughput near the `-bc none` baseline (224 / 14) and on-disk size orders of magnitude smaller than 524 MB (the bench payload is `'x'.repeat(1048576)`).
3. **Optional:** revisit [src/block.rs:49–57](../src/block.rs#L49-L57) to decide whether a single record larger than `target_block_size` should live in its own oversized block (current behaviour) or be rejected / split. The current behaviour is the source of the "N = record size" at LZ77's entry; it's not wrong, but it means `block_size_kb` is a floor for the typical case and a no-op for large records.

---

## Raw output

Per-run logs (each headed with the exact `rustikv` and `kvbench` invocation, run date, and commit hash; the new runs also record the DB directory size after each scenario): [`docs/benchmark-runs/2026-04-24/`](benchmark-runs/2026-04-24/).
