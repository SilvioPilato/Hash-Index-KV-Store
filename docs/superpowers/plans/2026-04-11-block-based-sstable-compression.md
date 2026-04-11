# Block-Based SSTable Compression Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement block-based SSTable format with varint-based LZ77 compression for the LSM engine, replacing the current record-by-record format.

**Architecture:** 
- New `lz77` module: varint-based LZ77 encoder/decoder (sliding window, match-finding)
- New `block` module: block writer (fills blocks, compresses) and block reader (decompresses, iterates)
- Refactor `sstable.rs`: rewrite `from_memtable()`, `rebuild_index()`, `get()`, `iter()` to work with blocks
- Sparse index: reorganized to point to block offsets instead of record offsets
- CLI flags: `--block-size-kb`, `--block-compression` in `Settings`
- Breaking change: old uncompressed SSTables are not supported

**Tech Stack:** Rust, hand-rolled LZ77 (no external compression crates), varint encoding

---

## File Structure

**New files:**
- `src/lz77.rs` - LZ77 encoder/decoder (sliding window, match-finding, varint encoding)
- `src/block.rs` - BlockWriter (buffers records, manages block boundaries, compresses) and BlockReader (decompresses, iterates records)
- `tests/lz77.rs` - LZ77 codec tests (roundtrip, various data patterns)
- `tests/block_sstable.rs` - Block I/O tests (read/write, spanning, mixed compression)

**Modified files:**
- `src/sstable.rs` - Rewrite SSTable to use block format (from_memtable, rebuild_index, get, iter)
- `src/settings.rs` - Add `block_size_kb`, `block_compression` CLI flags
- `src/lsmengine.rs` - Minor: ensure compaction works with new SSTable format (should be transparent)

---

## Task Breakdown

### Task 1: Set up new modules and CLI flags

**Files:**
- Create: `src/lz77.rs` (skeleton)
- Create: `src/block.rs` (skeleton)
- Modify: `src/settings.rs`
- Modify: `src/lib.rs` (add module declarations)
- Modify: `src/main.rs` (wire in new flags)

- [ ] **Step 1: Create `src/lz77.rs` with module structure**

Create the file with public structs and function signatures (no implementation yet):

```rust
// src/lz77.rs

/// LZ77 encoder for compression
pub struct Lz77Encoder {
    window_size: usize,
}

/// LZ77 decoder for decompression
pub struct Lz77Decoder;

impl Lz77Encoder {
    /// Create a new encoder with 32 KB sliding window
    pub fn new() -> Self {
        Self { window_size: 32_000 }
    }

    /// Compress data using LZ77 with varint encoding
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        unimplemented!()
    }
}

impl Lz77Decoder {
    /// Decompress varint-encoded LZ77 data
    pub fn decode(compressed: &[u8]) -> std::io::Result<Vec<u8>> {
        unimplemented!()
    }
}
```

- [ ] **Step 2: Create `src/block.rs` with module structure**

```rust
// src/block.rs

use std::io::{self, Read, Write};
use crate::record::Record;

/// 9-byte block header
#[derive(Debug)]
pub struct BlockHeader {
    pub uncompressed_size: u32,
    pub compressed_size: u32,
    pub compression_flag: u8,  // 0 = none, 1 = LZ77
}

impl BlockHeader {
    const SIZE: usize = 9;

    /// Serialize header to bytes (big-endian)
    pub fn to_bytes(&self) -> [u8; 9] {
        unimplemented!()
    }

    /// Deserialize header from bytes
    pub fn from_bytes(bytes: &[u8; 9]) -> Self {
        unimplemented!()
    }
}

/// Writes records to blocks with optional compression
pub struct BlockWriter {
    target_block_size: usize,
    compression_enabled: bool,
}

impl BlockWriter {
    pub fn new(target_block_size: usize, compression_enabled: bool) -> Self {
        Self {
            target_block_size,
            compression_enabled,
        }
    }

    /// Add a record to the current block, flushing if needed
    pub fn add_record(&mut self, record: &Record) -> io::Result<Option<Vec<u8>>> {
        unimplemented!()
    }

    /// Flush remaining block
    pub fn flush(&mut self) -> io::Result<Option<Vec<u8>>> {
        unimplemented!()
    }
}

/// Reads and decompresses blocks
pub struct BlockReader;

impl BlockReader {
    /// Read a block from file at offset, decompress if needed
    pub fn read_block(file: &mut dyn Read, header: &BlockHeader) -> io::Result<Vec<u8>> {
        unimplemented!()
    }
}
```

- [ ] **Step 3: Add CLI flags to `src/settings.rs`**

Modify the `Settings` struct to include:

```rust
pub struct Settings {
    // ... existing fields ...
    pub block_size_kb: usize,           // default: 4
    pub block_compression: bool,        // default: true (enable LZ77)
}
```

And update `parse_args()` to handle:
- `--block-size-kb <N>` (default 4, range 1-1024)
- `--block-compression <none|lz77>` (default lz77)

- [ ] **Step 4: Add module declarations to `src/lib.rs`**

```rust
pub mod lz77;
pub mod block;
```

- [ ] **Step 5: Wire flags into `main.rs`**

Pass `settings.block_size_kb` and `settings.block_compression` to the LSM engine (or store in `StorageEngine` state).

- [ ] **Step 6: Run `cargo check`**

Verify the code compiles (even though functions are unimplemented).

```bash
cargo check
```

Expected: No errors, only warnings about unimplemented code.

- [ ] **Step 7: Commit**

```bash
git add src/lz77.rs src/block.rs src/settings.rs src/lib.rs src/main.rs
git commit -m "feat: add block compression module stubs and CLI flags"
```

---

### Task 2: Implement LZ77 codec (encoder/decoder)

**Files:**
- Modify: `src/lz77.rs` (full implementation)
- Create: `tests/lz77.rs`

- [ ] **Step 1: Write LZ77 roundtrip test**

```rust
// tests/lz77.rs

use kv_store::lz77::*;

#[test]
fn test_lz77_roundtrip_simple() {
    let encoder = Lz77Encoder::new();
    let original = b"hello world";
    let compressed = encoder.encode(original);
    let decompressed = Lz77Decoder::decode(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_roundtrip_with_repetition() {
    let encoder = Lz77Encoder::new();
    let original = b"the quick brown fox jumps over the lazy dog";
    let compressed = encoder.encode(original);
    let decompressed = Lz77Decoder::decode(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_roundtrip_large_data() {
    let encoder = Lz77Encoder::new();
    let original: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
    let compressed = encoder.encode(&original);
    let decompressed = Lz77Decoder::decode(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_empty() {
    let encoder = Lz77Encoder::new();
    let original = b"";
    let compressed = encoder.encode(original);
    let decompressed = Lz77Decoder::decode(&compressed).unwrap();
    assert_eq!(decompressed, original);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test test_lz77_roundtrip_simple
```

Expected: FAIL - `encode` is not implemented

- [ ] **Step 3: Implement LZ77 encoder**

In `src/lz77.rs`, implement `Lz77Encoder::encode()`:

```rust
impl Lz77Encoder {
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        let mut pos = 0;

        while pos < data.len() {
            // Find longest match in sliding window
            let best_match = self.find_longest_match(data, pos);

            if best_match.length >= 3 {
                // Emit match: type=1, varint(offset), varint(length)
                output.push(1);
                self.encode_varint(best_match.offset as u32, &mut output);
                self.encode_varint(best_match.length as u32, &mut output);
                pos += best_match.length;
            } else {
                // Emit literal: type=0, byte
                output.push(0);
                output.push(data[pos]);
                pos += 1;
            }
        }

        output
    }

    fn find_longest_match(&self, data: &[u8], pos: usize) -> Match {
        let mut best_match = Match { offset: 0, length: 0 };
        let window_start = if pos > self.window_size {
            pos - self.window_size
        } else {
            0
        };

        for candidate in window_start..pos {
            let mut len = 0;
            while pos + len < data.len()
                && len < 258
                && data[candidate + len] == data[pos + len]
            {
                len += 1;
            }

            if len > best_match.length {
                best_match = Match {
                    offset: pos - candidate,
                    length: len,
                };
            }
        }

        best_match
    }

    fn encode_varint(&self, mut value: u32, output: &mut Vec<u8>) {
        while value >= 128 {
            output.push((value & 0x7F | 0x80) as u8);
            value >>= 7;
        }
        output.push((value & 0x7F) as u8);
    }
}

struct Match {
    offset: usize,
    length: usize,
}
```

- [ ] **Step 4: Implement LZ77 decoder**

```rust
impl Lz77Decoder {
    pub fn decode(compressed: &[u8]) -> io::Result<Vec<u8>> {
        let mut output = Vec::new();
        let mut pos = 0;

        while pos < compressed.len() {
            match compressed[pos] {
                0 => {
                    // Literal
                    pos += 1;
                    if pos >= compressed.len() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "truncated literal",
                        ));
                    }
                    output.push(compressed[pos]);
                    pos += 1;
                }
                1 => {
                    // Match reference
                    pos += 1;
                    let (offset, offset_len) = Self::decode_varint(&compressed[pos..])?;
                    pos += offset_len;
                    let (length, length_len) = Self::decode_varint(&compressed[pos..])?;
                    pos += length_len;

                    let source_pos = output.len() - offset as usize;
                    for i in 0..length as usize {
                        output.push(output[source_pos + i]);
                    }
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid token type",
                    ))
                }
            }
        }

        Ok(output)
    }

    fn decode_varint(data: &[u8]) -> io::Result<(u32, usize)> {
        let mut value: u32 = 0;
        let mut shift = 0;
        let mut pos = 0;

        loop {
            if pos >= data.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "truncated varint",
                ));
            }
            let byte = data[pos];
            pos += 1;
            value |= ((byte & 0x7F) as u32) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
        }

        Ok((value, pos))
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test test_lz77
```

Expected: All LZ77 tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/lz77.rs tests/lz77.rs
git commit -m "feat: implement LZ77 encoder/decoder with varint encoding"
```

---

### Task 3: Implement block I/O (BlockHeader, BlockWriter, BlockReader)

**Files:**
- Modify: `src/block.rs` (full implementation)
- Create: `tests/block_sstable.rs`

- [ ] **Step 1: Write block header tests**

```rust
// tests/block_sstable.rs

use kv_store::block::BlockHeader;

#[test]
fn test_block_header_serialization() {
    let header = BlockHeader {
        uncompressed_size: 1024,
        compressed_size: 512,
        compression_flag: 1,
    };

    let bytes = header.to_bytes();
    assert_eq!(bytes.len(), 9);

    let restored = BlockHeader::from_bytes(&bytes);
    assert_eq!(restored.uncompressed_size, 1024);
    assert_eq!(restored.compressed_size, 512);
    assert_eq!(restored.compression_flag, 1);
}

#[test]
fn test_block_header_big_endian() {
    let header = BlockHeader {
        uncompressed_size: 0x12345678,
        compressed_size: 0x9ABCDEF0,
        compression_flag: 0,
    };

    let bytes = header.to_bytes();
    // First 4 bytes: 0x12345678 in big-endian
    assert_eq!(bytes[0], 0x12);
    assert_eq!(bytes[1], 0x34);
    assert_eq!(bytes[2], 0x56);
    assert_eq!(bytes[3], 0x78);
    // Next 4 bytes: 0x9ABCDEF0 in big-endian
    assert_eq!(bytes[4], 0x9A);
    assert_eq!(bytes[5], 0xBC);
    assert_eq!(bytes[6], 0xDE);
    assert_eq!(bytes[7], 0xF0);
    // Last byte: compression_flag
    assert_eq!(bytes[8], 0);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test test_block_header
```

Expected: FAIL - `to_bytes` not implemented

- [ ] **Step 3: Implement BlockHeader serialization**

```rust
impl BlockHeader {
    pub fn to_bytes(&self) -> [u8; 9] {
        let mut bytes = [0u8; 9];
        bytes[0..4].copy_from_slice(&self.uncompressed_size.to_be_bytes());
        bytes[4..8].copy_from_slice(&self.compressed_size.to_be_bytes());
        bytes[8] = self.compression_flag;
        bytes
    }

    pub fn from_bytes(bytes: &[u8; 9]) -> Self {
        let uncompressed_size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let compressed_size = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let compression_flag = bytes[8];

        Self {
            uncompressed_size,
            compressed_size,
            compression_flag,
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test test_block_header
```

Expected: PASS

- [ ] **Step 5: Write BlockWriter tests**

```rust
#[test]
fn test_block_writer_uncompressed() {
    use kv_store::record::{Record, RecordHeader};

    let mut writer = BlockWriter::new(100, false); // 100 byte target, no compression

    let record = Record {
        header: RecordHeader {
            crc32: 0,
            key_size: 3,
            value_size: 5,
            tombstone: false,
        },
        key: "key".to_string(),
        value: "value".to_string(),
    };

    // Add first record
    let block1 = writer.add_record(&record).unwrap();
    assert!(block1.is_none()); // Should fit in buffer

    // Keep adding until we exceed block size
    let mut block_produced = false;
    for _ in 0..10 {
        let block = writer.add_record(&record).unwrap();
        if block.is_some() {
            block_produced = true;
            break;
        }
    }

    assert!(block_produced, "Should produce a block when size exceeded");
}

#[test]
fn test_block_writer_flush() {
    use kv_store::record::{Record, RecordHeader};

    let mut writer = BlockWriter::new(4096, false);
    let record = Record {
        header: RecordHeader {
            crc32: 0,
            key_size: 3,
            value_size: 5,
            tombstone: false,
        },
        key: "key".to_string(),
        value: "value".to_string(),
    };

    writer.add_record(&record).unwrap();
    let block = writer.flush().unwrap();
    assert!(block.is_some(), "Flush should produce remaining block");
}
```

- [ ] **Step 6: Run test to verify it fails**

```bash
cargo test test_block_writer
```

Expected: FAIL - BlockWriter not fully implemented

- [ ] **Step 7: Implement BlockWriter**

```rust
impl BlockWriter {
    pub fn new(target_block_size: usize, compression_enabled: bool) -> Self {
        Self {
            target_block_size,
            compression_enabled,
        }
    }

    pub fn add_record(&mut self, record: &Record) -> io::Result<Option<Vec<u8>>> {
        // (Implementation detail: maintain internal buffer, check size, flush when needed)
        unimplemented!()
    }

    pub fn flush(&mut self) -> io::Result<Option<Vec<u8>>> {
        unimplemented!()
    }

    fn write_block(&self, records_data: &[u8]) -> io::Result<Vec<u8>> {
        let uncompressed_size = records_data.len() as u32;
        let (compressed_data, compression_flag) = if self.compression_enabled {
            let encoder = crate::lz77::Lz77Encoder::new();
            let compressed = encoder.encode(records_data);
            (compressed, 1u8)
        } else {
            (records_data.to_vec(), 0u8)
        };

        let compressed_size = compressed_data.len() as u32;
        let header = BlockHeader {
            uncompressed_size,
            compressed_size,
            compression_flag,
        };

        let mut block = Vec::new();
        block.extend_from_slice(&header.to_bytes());
        block.extend_from_slice(&compressed_data);
        Ok(block)
    }
}
```

- [ ] **Step 8: Implement BlockReader**

```rust
impl BlockReader {
    pub fn read_block(file: &mut dyn Read, header: &BlockHeader) -> io::Result<Vec<u8>> {
        let mut compressed_data = vec![0u8; header.compressed_size as usize];
        file.read_exact(&mut compressed_data)?;

        if header.compression_flag == 0 {
            // Uncompressed
            Ok(compressed_data)
        } else if header.compression_flag == 1 {
            // LZ77 compressed
            crate::lz77::Lz77Decoder::decode(&compressed_data)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown compression flag",
            ))
        }
    }
}
```

- [ ] **Step 9: Run tests to verify they pass**

```bash
cargo test test_block
```

Expected: PASS

- [ ] **Step 10: Commit**

```bash
git add src/block.rs tests/block_sstable.rs
git commit -m "feat: implement block I/O with header serialization and decompression"
```

---

### Task 4: Rewrite SSTable to use blocks

**Files:**
- Modify: `src/sstable.rs` (major rewrite)

- [ ] **Step 1: Write test for block-based SSTable creation**

```rust
// Add to tests/block_sstable.rs

#[test]
fn test_sstable_from_memtable_blocks() {
    use kv_store::memtable::Memtable;
    use kv_store::sstable::SSTable;

    let mut memtable = Memtable::new();
    memtable.set("key1", "value1").unwrap();
    memtable.set("key2", "value2").unwrap();
    memtable.set("key3", "value3").unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    let sstable = SSTable::from_memtable(
        temp_dir.path().to_str().unwrap(),
        "test",
        &memtable,
        None,
    )
    .unwrap();

    // Verify sparse index is populated (block-level)
    assert!(!sstable.sparse_index.is_empty(), "Sparse index should have block entries");
    
    // Verify Bloom filter was built
    assert!(sstable.bloom.might_contain("key1"));
    assert!(sstable.bloom.might_contain("key2"));
    assert!(sstable.bloom.might_contain("key3"));
}

#[test]
fn test_sstable_get_from_blocks() {
    use kv_store::memtable::Memtable;
    use kv_store::sstable::SSTable;

    let mut memtable = Memtable::new();
    memtable.set("alpha", "apple").unwrap();
    memtable.set("bravo", "banana").unwrap();
    memtable.set("charlie", "cherry").unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    let sstable = SSTable::from_memtable(
        temp_dir.path().to_str().unwrap(),
        "test",
        &memtable,
        None,
    )
    .unwrap();

    // Test reads
    assert_eq!(sstable.get("alpha").unwrap(), Some(Some("apple".to_string())));
    assert_eq!(sstable.get("bravo").unwrap(), Some(Some("banana".to_string())));
    assert_eq!(sstable.get("charlie").unwrap(), Some(Some("cherry".to_string())));
    assert_eq!(sstable.get("delta").unwrap(), None);
}

#[test]
fn test_sstable_iter_blocks() {
    use kv_store::memtable::Memtable;
    use kv_store::sstable::SSTable;

    let mut memtable = Memtable::new();
    memtable.set("x", "1").unwrap();
    memtable.set("y", "2").unwrap();
    memtable.set("z", "3").unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    let sstable = SSTable::from_memtable(
        temp_dir.path().to_str().unwrap(),
        "test",
        &memtable,
        None,
    )
    .unwrap();

    let mut iter = sstable.iter().unwrap();
    let records: Vec<_> = iter.collect::<Result<_, _>>().unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].key, "x");
    assert_eq!(records[1].key, "y");
    assert_eq!(records[2].key, "z");
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test test_sstable_from_memtable_blocks
```

Expected: FAIL - needs rewrite

- [ ] **Step 3: Rewrite `SSTable::from_memtable()`**

The new implementation uses BlockWriter to fill blocks:

```rust
pub fn from_memtable(
    dir: &str,
    name: &str,
    memtable: &Memtable,
    level: Option<usize>,
) -> io::Result<SSTable> {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| io::Error::other("SystemTime error"))?
        .as_nanos() as u64;

    let filename = match level {
        Some(l) => format!("{}_L{}_{}.sst", name, l, timestamp),
        None => format!("{}_{}.sst", name, timestamp),
    };
    let path = PathBuf::from(dir).join(&filename);

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;

    let mut block_writer = crate::block::BlockWriter::new(4096, true); // TODO: use settings
    let mut sparse_index: Vec<(String, u64)> = Vec::new();
    let mut bloom_keys: Vec<String> = Vec::new();

    let mut file_offset = 0u64;
    let mut block_count = 0usize;

    for (key, opt_value) in memtable.entries().iter() {
        let (value, tombstone) = match opt_value {
            Some(v) => (v.to_string(), false),
            None => (String::new(), true),
        };

        let record = Record {
            header: RecordHeader {
                crc32: 0u32,
                key_size: key.len() as u64,
                value_size: value.len() as u64,
                tombstone,
            },
            key: key.to_string(),
            value,
        };

        bloom_keys.push(key.clone());

        if let Some(block_data) = block_writer.add_record(&record)? {
            // Block was produced, write it
            file.write_all(&block_data)?;
            
            // Record sparse index entry (first key of block)
            if block_count % 1 == 0 {  // Sample every block
                sparse_index.push((key.clone(), file_offset));
            }

            file_offset += block_data.len() as u64;
            block_count += 1;
        }
    }

    // Flush remaining block
    if let Some(block_data) = block_writer.flush()? {
        file.write_all(&block_data)?;
        if block_count % 1 == 0 {
            sparse_index.push((
                memtable.entries().last().unwrap().0.clone(),
                file_offset,
            ));
        }
    }

    // Rebuild Bloom filter and verify index
    let bloom_bytes = (bloom_keys.len() * BLOOM_BITS_PER_KEY).div_ceil(8);
    let mut bloom = BloomFilter::new(bloom_bytes.max(1), BLOOM_HASH_COUNT);
    for key in &bloom_keys {
        bloom.insert(key);
    }

    let min = sparse_index.first().cloned();
    let max = bloom_keys.last().map(|k| {
        let pos = sparse_index.partition_point(|(sk, _)| sk.as_str() <= k.as_str());
        let offset = if pos > 0 { sparse_index[pos - 1].1 } else { 0 };
        (k.clone(), offset)
    });

    Ok(SSTable {
        path,
        timestamp,
        name: name.to_string(),
        level,
        sparse_index,
        bloom,
        min,
        max,
    })
}
```

- [ ] **Step 4: Rewrite `SSTable::get()`**

New implementation decompresses blocks on demand:

```rust
pub fn get(&self, key: &str) -> io::Result<Option<Option<String>>> {
    if !self.bloom.might_contain(key) {
        return Ok(None);
    }

    let offset = self.get_offset(key);
    let file = OpenOptions::new().read(true).open(&self.path)?;
    let mut reader = BufReader::new(file);
    reader.seek(io::SeekFrom::Start(offset))?;

    // Read blocks until we find the key
    loop {
        let mut header_bytes = [0u8; 9];
        match reader.read_exact(&mut header_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }

        let header = crate::block::BlockHeader::from_bytes(&header_bytes);
        let block_data = crate::block::BlockReader::read_block(&mut reader, &header)?;

        // Iterate records in decompressed block
        let mut record_reader = &block_data[..];
        loop {
            match Record::read_next(&mut record_reader) {
                Ok(record) => {
                    match record.key.as_str().cmp(key) {
                        std::cmp::Ordering::Greater => return Ok(None),
                        std::cmp::Ordering::Equal => {
                            let value = if record.header.tombstone {
                                None
                            } else {
                                Some(record.value)
                            };
                            return Ok(Some(value));
                        }
                        std::cmp::Ordering::Less => continue,
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            }
        }

        // Not found in this block, try next block
    }
}
```

- [ ] **Step 5: Rewrite `SSTable::iter()`**

```rust
pub fn iter(&self) -> io::Result<SSTableBlockIter> {
    let file = OpenOptions::new().read(true).open(&self.path)?;
    Ok(SSTableBlockIter {
        reader: BufReader::new(file),
        done: false,
    })
}
```

Add new iterator struct:

```rust
pub struct SSTableBlockIter {
    reader: BufReader<File>,
    done: bool,
}

impl Iterator for SSTableBlockIter {
    type Item = io::Result<Record>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        // Read block header
        let mut header_bytes = [0u8; 9];
        match self.reader.read_exact(&mut header_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                self.done = true;
                return None;
            }
            Err(e) => return Some(Err(e)),
        }

        let header = crate::block::BlockHeader::from_bytes(&header_bytes);
        let block_data = match crate::block::BlockReader::read_block(&mut self.reader, &header) {
            Ok(data) => data,
            Err(e) => return Some(Err(e)),
        };

        // TODO: Iterate through records in block_data
        // This requires maintaining state across iterator calls
        unimplemented!()
    }
}
```

- [ ] **Step 6: Rewrite `SSTable::rebuild_index()`**

```rust
pub fn rebuild_index(&mut self) -> io::Result<()> {
    let file = OpenOptions::new().read(true).open(&self.path)?;
    let mut reader = BufReader::new(file);
    let mut sparse_index: Vec<(String, u64)> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    let mut block_count = 0usize;

    loop {
        let offset = reader.stream_position()?;
        let mut header_bytes = [0u8; 9];

        match reader.read_exact(&mut header_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }

        let header = crate::block::BlockHeader::from_bytes(&header_bytes);
        let block_data = crate::block::BlockReader::read_block(&mut reader, &header)?;

        // Extract keys from decompressed block
        let mut record_reader = &block_data[..];
        let mut first_key_in_block = None;

        loop {
            match Record::read_next(&mut record_reader) {
                Ok(record) => {
                    if first_key_in_block.is_none() {
                        first_key_in_block = Some(record.key.clone());
                    }
                    keys.push(record.key);
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            }
        }

        if block_count % 1 == 0 {
            if let Some(first_key) = first_key_in_block {
                sparse_index.push((first_key, offset));
            }
        }

        block_count += 1;
    }

    let bloom_bytes = (keys.len() * BLOOM_BITS_PER_KEY).div_ceil(8);
    let mut bloom = BloomFilter::new(bloom_bytes.max(1), BLOOM_HASH_COUNT);
    for key in &keys {
        bloom.insert(key);
    }

    self.min = sparse_index.first().cloned();
    self.max = keys.last().map(|k| {
        let pos = sparse_index.partition_point(|(sk, _)| sk.as_str() <= k.as_str());
        let offset = if pos > 0 { sparse_index[pos - 1].1 } else { 0 };
        (k.clone(), offset)
    });

    self.sparse_index = sparse_index;
    self.bloom = bloom;
    Ok(())
}
```

- [ ] **Step 7: Run tests to verify they pass**

```bash
cargo test test_sstable_from_memtable_blocks
cargo test test_sstable_get_from_blocks
```

Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add src/sstable.rs tests/block_sstable.rs
git commit -m "feat: rewrite SSTable to use block-based format with decompression"
```

---

### Task 5: Integration tests and validation

**Files:**
- Modify: `tests/block_sstable.rs`

- [ ] **Step 1: Write integration test (full LSM flow)**

```rust
#[test]
fn test_lsm_with_block_compression() {
    use kv_store::lsmengine::LsmEngine;
    use std::sync::Arc;

    let temp_dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(
        LsmEngine::new(temp_dir.path().to_str().unwrap(), "test").unwrap()
    );

    // Write some data
    engine.set("k1", "v1").unwrap();
    engine.set("k2", "v2").unwrap();
    engine.set("k3", "v3").unwrap();

    // Flush memtable to SSTable
    engine.flush_memtable().unwrap();

    // Read it back
    assert_eq!(engine.get("k1").unwrap(), Some(("k1".to_string(), "v1".to_string())));
    assert_eq!(engine.get("k2").unwrap(), Some(("k2".to_string(), "v2".to_string())));
    assert_eq!(engine.get("k3").unwrap(), Some(("k3".to_string(), "v3".to_string())));
}

#[test]
fn test_lsm_with_block_compression_and_compaction() {
    use kv_store::lsmengine::LsmEngine;
    use std::sync::Arc;

    let temp_dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(
        LsmEngine::new(temp_dir.path().to_str().unwrap(), "test").unwrap()
    );

    for i in 0..100 {
        engine.set(&format!("key{:03}", i), &format!("value{}", i)).unwrap();
    }

    // Force flush and compaction
    engine.flush_memtable().unwrap();
    engine.compact().unwrap();

    // Verify data is still readable
    for i in 0..100 {
        let result = engine.get(&format!("key{:03}", i)).unwrap();
        assert_eq!(
            result,
            Some((format!("key{:03}", i), format!("value{}", i)))
        );
    }
}
```

- [ ] **Step 2: Run integration tests**

```bash
cargo test test_lsm_with_block_compression
```

Expected: PASS

- [ ] **Step 3: Run full test suite**

```bash
cargo test
```

Expected: All tests PASS (or at least no regressions from block changes)

- [ ] **Step 4: Commit**

```bash
git add tests/block_sstable.rs
git commit -m "feat: add integration tests for block-based LSM"
```

---

### Task 6: Pre-commit checklist and final validation

**Files:** All modified files

- [ ] **Step 1: Run `cargo fmt`**

```bash
cargo fmt
```

Expected: Code is formatted

- [ ] **Step 2: Run `cargo clippy -- -D warnings`**

```bash
cargo clippy -- -D warnings
```

Expected: Zero warnings

- [ ] **Step 3: Run full test suite**

```bash
cargo test --all
```

Expected: All tests PASS

- [ ] **Step 4: Manual smoke test with REPL**

```bash
cargo run --bin rustikli &
```

Commands:
```
SET test_key "test_value"
GET test_key
COMPACT
GET test_key
```

Expected: All operations work, data is correctly stored and retrieved

- [ ] **Step 5: Final commit (if needed)**

If any fixes were made in steps 1-4:

```bash
git add -A
git commit -m "style: format and fix clippy warnings"
```

---

## Testing Strategy Summary

| Layer | Tests | Location |
|-------|-------|----------|
| LZ77 codec | Roundtrip, empty data, large data | `tests/lz77.rs` |
| Block I/O | Header serialization, read/write | `tests/block_sstable.rs` |
| SSTable | from_memtable, get, iter | `tests/block_sstable.rs` |
| LSM integration | Full workflows, compaction | `tests/block_sstable.rs` |

---

## Rollback/Abort Plan

If major issues are discovered:
1. Revert to last working commit before Task 1
2. Branch off and investigate in isolation
3. Do not merge back to main until root cause is clear

---

## Notes

- **Settings integration**: Tasks 4-6 assume `block_size_kb` and `block_compression` are available from `Settings`. Wire these into `from_memtable()` and `BlockWriter` creation.
- **Iterator complexity**: The new `SSTableBlockIter` is complex (needs to buffer decompressed blocks). May want to simplify in future tasks.
- **Error handling**: All LZ77/decompression errors should fail hard (no silent skipping).
- **Backward compatibility**: Old uncompressed SSTables will not be readable. This is a breaking change (intentional).
