# rustikv as a Telemetry Store — Experimental Use Case

## What it is

rustikv is a didactic TCP key-value store with two pluggable storage engines:

- **KV (Bitcask)** — hash index in memory, O(1) reads, append-only writes. Best for random access by exact key.
- **LSM** — write-optimized, memtable flushed to sorted SSTables. Supports `RANGE` queries. Best for sequential/time-series workloads.

For telemetry, **LSM is the right engine** — its write path is append-only and absorbs bursts well, and `RANGE` enables time-window queries when you design keys with timestamps (e.g. `cpu:host1:2026-05-10T12:00:00`).

## How it fits telemetry today

| Need | Status |
|------|--------|
| High write throughput | LSM engine handles this natively |
| Time-range queries | `RANGE <start> <end>` on LSM, works now |
| Batch ingestion | `MSET` reduces round trips, works now |
| Storage efficiency | LZ77 block compression on SSTables, works now |
| Fast key-existence checks | Bloom filters per SSTable, works now |

## What's missing and why it matters

**1. TTL (`#56`) — top priority**
Without expiry, the store grows unbounded. Telemetry data is inherently time-limited (you don't need last year's CPU metrics). This is the single biggest blocker for any sustained experiment.

**2. INCR (`#55`) — second priority**
Counter and rate metrics (request counts, error counts, latency buckets) require atomic increment. Without it you'd do read-modify-write on every tick — racy and expensive.

**3. PREFIX (`#50`) — third priority**
Enables querying an entire metric namespace (`cpu:host1:*`) without knowing exact key bounds. Makes `RANGE` much more ergonomic in practice.

**4. COUNT (`#51`) — nice to have**
Lightweight cardinality check — how many series exist under a prefix — without pulling all values to the client.

**5. Server-side aggregation — new task needed**
For SUM/AVG/MIN/MAX over a time range. Currently you'd have to read all raw values and aggregate client-side, which is the main scalability pain point. Not blocking for experimentation, but worth a task eventually.

## What to skip for now

Replication, consistent hashing, and partitioning are all single-node concerns for this experiment. Downsampling can be handled client-side. These are future concerns if the experiment outgrows a single node.

## Suggested path

```
TTL → INCR → PREFIX → (COUNT) → (server-side aggregation)
```

Implement in that order. The first two unlock the majority of useful telemetry patterns; the rest are progressive improvements.
