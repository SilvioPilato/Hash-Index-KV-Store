# Block-Based SSTable Format with LZ77 Compression (Task #29)

**Date**: 2026-04-11  
**Author**: Claude + Silvio Pilato  
**Status**: Design Review

## Executive Summary

This specification describes a block-based SSTable format for the LSM engine that partitions data into fixed-size blocks (default 4 KB, configurable) with optional per-block LZ77 compression. The sparse index is reorganized to point to block boundaries rather than record offsets. Records may span block boundaries. This design teaches data layout optimization, compression fundamentals, and production patterns used in RocksDB and similar databases.

## Motivation

- **DDIA Chapter 3**: LSM-trees benefit from compression at the storage layer, particularly when data is large.
- **Production practice**: RocksDB, LevelDB, and other real databases use block-based formats with per-block compression.
- **Educational value**: Students learn the separation between record format and layout optimization, and how compression works as an orthogonal concern.
- **Performance**: Space savings from compression reduce I/O and memory usage; block-level decompression is cheaper than full-file decompression.

## Design Goals

1. **Educational clarity**: Block format is simple enough to understand in one sitting; compression is independently implementable.
2. **Production realism**: Matches how real databases organize data; students see patterns they'll encounter in industry code.
3. **Flexibility**: Compression is optional (via CLI flag), allowing benchmarking and comparison.
4. **Backward incompatibility accepted**: This is a breaking change; old uncompressed record-by-record SSTables are not supported.

## Architecture

### Block Structure

Each block consists of:

```
Block Header (16 bytes, big-endian):
  byte 0-3:   uncompressed_size (u32)   — size of data before compression
  byte 4-7:   compressed_size (u32)     — size of data after compression
  byte 8:     compression_flag (u8)     — 0 = uncompressed, 1 = LZ77-compressed
  byte 9-15:  reserved (7 bytes)        — for future use (padding to 16 bytes)

Block Body:
  If compression_flag == 0:
    Raw sequence of Record objects (existing Record format).
  If compression_flag == 1:
    LZ77-compressed byte stream (output of LZ77 encoder).
```

**Rationale for header design**:
- 16 bytes allows simple alignment and efficient I/O.
- `uncompressed_size` is needed during decompression (allocate buffer).
- `compressed_size` is needed during read (know how many bytes to consume).
- `compression_flag` allows mixed compression strategies (useful for future extensions).
- Reserved bytes allow per-block checksums (e.g., CRC) in the future without breaking the format.

### SSTable On-Disk Layout

```
SSTable file:
┌─────────────────────────────────────────────┐
│ Block 0 (Header + Data, possibly compressed)│
├─────────────────────────────────────────────┤
│ Block 1 (Header + Data, possibly compressed)│
├─────────────────────────────────────────────┤
│ ...                                         │
├─────────────────────────────────────────────┤
│ Block N (Header + Data, possibly compressed)│
└─────────────────────────────────────────────┘
```

**Index Storage**: The sparse index is **not persisted in the file**. It is rebuilt on startup by scanning all blocks (similar to current `rebuild_index()` behavior). This simplifies the on-disk format and matches existing patterns.

### Sparse Index

**Structure**: `Vec<(String, u64)>` where each entry is `(first_key_in_block, block_start_offset)`.

**Sampling strategy**: 
- Sample every Nth block (configurable, default = 1, meaning every block).
- For Nth = 1 (dense), we have full precision. For Nth = 16 (coarse), we sample every 16th block.
- At minimum, the first block is always indexed.
- Built during write (track sampled blocks) and verified by `rebuild_index()` on startup.

**Lookup flow**:
1. Binary search sparse index on key name → find the block to start scanning.
2. If key < first key in index, start at block 0.
3. Open SSTable file, seek to block offset.
4. Read block header, decompress if needed.
5. Scan decompressed records for target key.
6. If key not found in current block, try next block (blocks are ordered; records don't split across blocks).

**Rationale**: Block-level indexing is standard in production databases (RocksDB, LevelDB). It aligns with the compression boundary and avoids needing to track record offsets within compressed data. Since records don't span blocks, the index gives exact block boundaries.

### Records Not Spanning Blocks

When filling a block during write:

```rust
loop {
    let record = next_record();
    if current_block_size + record.size_on_disk() > block_size_kb {
        // Block would overflow; close it and start a new one.
        flush_and_compress_block();
        start_new_block();
    }
    // Record now fits in current block (or is alone in new block).
    append_record_to_block(record);
}
```

**Why no spanning**:

- Records are never split across block boundaries.
- Simplifies decompression: read block header, decompress, iterate complete records.
- Aligns with production database patterns (LevelDB, RocksDB).
- Block-level sparse index is exact: each sampled block contains specific key ranges.

**Edge case**: If a single record is larger than the block size, it occupies its own block alone.

### Bloom Filter

**Remains unchanged**: Per-SSTable Bloom filter, rebuilt on startup by scanning all blocks.

**Rationale**: Bloom filters benefit from global statistics (false positive rate depends on total key count). Per-block Bloom filters would add complexity and memory overhead for marginal gain.

### LZ77 Compression Implementation

**Hand-rolled LZ77 algorithm**:
- **Sliding window**: 32 KB backward reference window.
- **Match length**: 3 to 258 bytes (standard LZ77 range).
- **Lazy matching** (optional optimization): Try extending matches before emitting.
- **Output format**: Literals and (offset, length) tuples, encoded compactly (see below).

**Encoding format** (varint/byte-oriented):
```
Control byte approach:
  - Each control byte describes the next 8 operations.
  - Bit = 0: literal byte follows.
  - Bit = 1: match token follows (2 or 3 bytes: offset_hi, offset_lo, length).

OR simpler approach:
  - Varint-encode (offset, length) pairs for matches.
  - Literal runs prefixed with length count.
```

**Choice**: Start with a simple approach (literal bytes + varint-encoded matches), optimize later if needed.

### CLI Configuration

**New flags**:
- `--block-size-kb <N>` — Block size in KB (default: 4). Range: [1, 1024].
- `--block-compression <none|lz77>` — Compression algorithm (default: `lz77`).

**Stored in `Settings`**: Both flags are part of engine configuration so they're consistent across runs.

### Backward Compatibility

**Breaking change**: 
- Old uncompressed record-by-record SSTables (from before this task) are **not supported**.
- On startup, if old `.sst` files exist in the data directory, they will be ignored (or an error is logged).
- User data must be migrated: re-compact the old SSTables or delete them and restart.

**Rationale**: Block format is fundamentally different. Supporting both would require dual read paths and complexity. For an educational project, a clean break is acceptable.

### Error Handling

**Decompression failures**:
- If LZ77 decompression fails (corrupt compressed data, truncated block), return an I/O error. Do not silently skip the block.
- Rationale: Matches existing behavior (CRC validation on records fails hard).

**Missing block metadata**:
- If block header is malformed or truncated, return an I/O error.
- If `compressed_size` > file size from that offset, return an error.

### Testing Strategy

**Unit tests** (`tests/block_sstable.rs` or similar):
1. **Block I/O**: Write a block with records, compress, decompress, verify records match.
2. **LZ77 codec**: Compress → decompress → verify roundtrip for various data patterns.
3. **Sparse index**: Binary search correctness with various sampling intervals.
4. **Record spanning**: Write records that intentionally span blocks, verify correct reads.
5. **Mixed compression**: Write some blocks uncompressed, others compressed; verify reads.
6. **Bloom filter rebuild**: Scan blocks, rebuild Bloom filter, verify lookups.

**Integration tests**:
1. Create an LSM engine with block-compressed SSTables, perform set/get/delete, verify correctness.
2. Compact multiple SSTables with block format, verify resulting blocks are correct.
3. Benchmark: compare throughput and file size with/without compression.

### Implementation Scope

**In scope**:
- Block format and header (16 bytes).
- LZ77 encoder/decoder (hand-rolled).
- Block writer (fill blocks, compress, write header + body).
- Block reader (read header, decompress, iterate records).
- Sparse index reorganization (per-block).
- SSTable rewrite to use blocks (from_memtable, rebuild_index, get, iter).
- CLI flags and Settings.
- Tests.

**Out of scope**:
- Per-block Bloom filters (too complex for now; remains per-SSTable).
- Per-block checksums (can be added to the reserved bytes later).
- Compression algorithm selection at block level (blocks are all the same algorithm).
- SIMD or advanced LZ77 optimizations (keep it simple for learning).
- Mmap for block reads (separate task #28).

## Data Flow

### Writing (Memtable → SSTable)

```
from_memtable(memtable):
  1. Create a block writer with target block size (e.g., 4 KB).
  2. For each (key, value) in memtable.entries():
     a. Serialize as Record (existing format).
     b. Check if record fits in current block (block_size + record.size_on_disk() <= target_size).
     c. If record does NOT fit:
        i. Close current block: compress if enabled, write block header + body to file.
        ii. Track first key of closed block for sparse index.
        iii. Start new block.
     d. Append record to current block buffer.
  3. Flush remaining block (if any).
  4. Rebuild sparse index and Bloom filter by scanning all blocks in the SSTable file.
  5. Return SSTable.
```

**Key point**: Records never span blocks. If a record doesn't fit in the current block, the block is closed and the record is placed entirely in the next block.

### Reading (SSTable → Records)

```
get(key):
  1. Bloom filter check: if key not in filter, return None.
  2. Sparse index lookup: binary search to find starting block.
  3. Open SSTable file, seek to block offset.
  4. Read block header (16 bytes).
  5. Read block body (compressed_size bytes).
  6. If compression_flag == 1: decompress body.
  7. Iterate decompressed records for key.
  8. If found, return value.
  9. If not found and more blocks exist, continue to next block.
  10. Return None.

iter():
  1. Iterate blocks sequentially.
  2. For each block: read header, decompress (if needed), iterate records.
  3. Yield records in order.
```

### Compaction (Merge SSTables)

Compaction merges multiple SSTables into a single new SSTable. With blocks, the flow is:

```
compact(sst_list):
  1. Open all SSTables and create block iterators.
  2. Merge-sort all blocks by key (LSM standard pattern).
  3. Use a block writer (as in from_memtable) to write results.
  4. For each merged record:
     a. Serialize as Record.
     b. Append to current block buffer (following the from_memtable logic).
     c. If block overflows, close block, write header + body, start new block.
  5. Flush remaining block.
  6. Rebuild sparse index and Bloom filter.
  7. Delete old SSTable files.
```

**Key insight**: Compaction doesn't "know" about blocks—it works at the record level. The block writer encapsulates block management and compression.

## File Format Examples

### Example Block (uncompressed, 3 records)

```
Hex dump:
00 00 00 4A                    # uncompressed_size = 74
00 00 00 4A                    # compressed_size = 74 (same, uncompressed)
00                             # compression_flag = 0 (none)
00 00 00 00 00 00 00           # reserved
[74 bytes of raw records...]
```

### Example Block (LZ77-compressed)

```
Hex dump:
00 00 01 00                    # uncompressed_size = 256
00 00 00 80                    # compressed_size = 128 (50% reduction)
01                             # compression_flag = 1 (LZ77)
00 00 00 00 00 00 00           # reserved
[128 bytes of LZ77-compressed data...]
```

## Appendix: LZ77 Algorithm Sketch

```
encode(data):
  window = [last 32 KB of encoded data]
  result = []
  pos = 0
  while pos < data.len():
    best_match = find_longest_match(data[pos..], window, max_length=258)
    if best_match.length >= 3:
      emit MATCH(best_match.offset, best_match.length)
      pos += best_match.length
    else:
      emit LITERAL(data[pos])
      pos += 1
  return encode(result)  # encode matches and literals into bytes

decode(compressed):
  result = []
  pos = 0
  while pos < compressed.len():
    token = read_next_token(compressed, pos)
    if token.is_literal:
      result.push(token.byte)
    else:  # is match
      offset = token.offset
      length = token.length
      for i in 0..length:
        result.push(result[result.len() - offset])
    pos += token.size
  return result
```

## Future Extensions

1. **Per-block checksums**: Add CRC32 in the reserved bytes.
2. **Dictionary compression**: Pre-shared compression dictionary for smaller files.
3. **Adaptive block size**: Vary block size based on data characteristics.
4. **Multiple compression algorithms**: Let users choose snappy, zstd, etc. (today: LZ77 only).
5. **Block-level caching**: Cache decompressed blocks in memory.

## References

- DDIA Chapter 3: Storage Engines (LSM-Trees, Compression)
- RocksDB: Block-based table format (https://github.com/facebook/rocksdb/wiki/Compression)
- LevelDB: SSTable format
