use crate::{lz77::Lz77, record::Record};
use std::io::{self, Read};

/// 9-byte block header
#[derive(Debug)]
pub struct BlockHeader {
    pub uncompressed_size: u32,
    pub stored_size: u32,
    pub compression_flag: u8, // 0 = none, 1 = LZ77
}

impl BlockHeader {
    /// Serialize header to bytes (big-endian)
    pub fn to_bytes(&self) -> [u8; 9] {
        let mut buf = [0u8; 9];
        buf[0..4].copy_from_slice(&self.uncompressed_size.to_be_bytes());
        buf[4..8].copy_from_slice(&self.stored_size.to_be_bytes());
        buf[8] = self.compression_flag;

        buf
    }

    /// Deserialize header from bytes
    pub fn from_bytes(bytes: &[u8; 9]) -> Self {
        Self {
            uncompressed_size: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
            stored_size: u32::from_be_bytes(bytes[4..8].try_into().unwrap()),
            compression_flag: bytes[8],
        }
    }
}

pub struct BlockWriter {
    target_block_size: usize,
    compression_enabled: bool,
    buffer: Vec<u8>,
}

impl BlockWriter {
    pub fn new(target_block_size: usize, compression_enabled: bool) -> Self {
        Self {
            target_block_size,
            compression_enabled,
            buffer: Vec::new(),
        }
    }

    /// Add a record to the current block, flushing if needed
    pub fn add_record(&mut self, record: &Record) -> io::Result<Option<Vec<u8>>> {
        let data = record.to_be_bytes();
        let mut flushed = None;
        if data.len() + self.buffer.len() > self.target_block_size {
            flushed = self.flush()?;
        }
        self.buffer.extend_from_slice(&data);
        Ok(flushed)
    }

    /// Flush remaining block
    pub fn flush(&mut self) -> io::Result<Option<Vec<u8>>> {
        if self.buffer.is_empty() {
            return Ok(None);
        }
        let mut buf = Vec::new();

        match self.compression_enabled {
            true => {
                let compressed = Lz77::encode(&self.buffer);
                let header = BlockHeader {
                    uncompressed_size: self.buffer.len() as u32,
                    stored_size: compressed.len() as u32,
                    compression_flag: 1,
                };
                buf.extend_from_slice(&header.to_bytes());
                buf.extend_from_slice(&compressed);
            }
            false => {
                let header = BlockHeader {
                    uncompressed_size: self.buffer.len() as u32,
                    stored_size: self.buffer.len() as u32,
                    compression_flag: 0,
                };
                buf.extend_from_slice(&header.to_bytes());
                buf.extend_from_slice(&self.buffer);
            }
        };
        self.buffer.clear();
        Ok(Some(buf))
    }
}

pub struct BlockReader;

impl BlockReader {
    /// Read a block from file at offset, decompress if needed
    pub fn read_block(file: &mut dyn Read, header: &BlockHeader) -> io::Result<Vec<u8>> {
        match header.compression_flag {
            0 => {
                let mut buf: Vec<u8> = vec![0; header.stored_size as usize];
                file.read_exact(&mut buf)?;
                Ok(buf)
            }
            1 => {
                let mut buf = vec![0; header.stored_size as usize];
                file.read_exact(&mut buf)?;
                Ok(Lz77::decode(&buf))
            }
            n => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid compression flag found {}", n),
            )),
        }
    }
}
