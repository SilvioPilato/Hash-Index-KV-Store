use std::{
    fs::File,
    io::{self, Error, ErrorKind, Read, Seek, SeekFrom, Write},
    vec,
};

use crate::crc::crc32;

pub const SIZE_FIELD_LEN: usize = 8;
pub const TOMBSTONE_LEN: usize = 1;
pub const RECORD_HEADER_LEN: usize = CRC_LEN + SIZE_FIELD_LEN * 2 + TOMBSTONE_LEN;
pub const MAX_KEY_SIZE: usize = 1_048_576;
pub const MAX_VALUE_SIZE: usize = 1_048_576 * 5;
pub const CRC_LEN: usize = 4;
/// The fixed-size header that precedes every key-value record on disk.
///
/// Layout (21 bytes, all integers big-endian):
///
/// ```text
/// | crc32 (4 B) | key_size (8 B) | value_size (8 B) | tombstone (1 B) |
/// ```
///
/// The CRC32 is computed over everything *after* the CRC field:
/// `key_size || value_size || tombstone || key || value`.
#[derive(Debug)]
pub struct RecordHeader {
    /// CRC32 checksum (IEEE polynomial) for integrity verification.
    pub crc32: u32,
    /// Length of the key in bytes.
    pub key_size: u64,
    /// Length of the value in bytes.
    pub value_size: u64,
    /// If `true`, this record marks a deletion (value is empty).
    pub tombstone: bool,
}

/// A complete key-value record as stored in a segment file.
///
/// On-disk layout:
///
/// ```text
/// | RecordHeader (21 B) | key (key_size B) | value (value_size B) |
/// ```
///
/// Use [`Record::append`] to write a record to a segment file,
/// and [`Record::read_next`] / [`Record::read_record_at`] to read one back.
#[derive(Debug)]
pub struct Record {
    pub header: RecordHeader,
    pub key: String,
    pub value: String,
}

impl Record {
    /// Returns the total number of bytes this record occupies on disk
    /// (header + key + value).
    pub fn size_on_disk(&self) -> u64 {
        RECORD_HEADER_LEN as u64 + self.header.key_size + self.header.value_size
    }

    /// Appends this record to the end of `file`, computing a fresh CRC32
    /// over the payload. Returns the byte offset where the record starts.
    pub fn append(&self, file: &mut File) -> io::Result<u64> {
        let current_eof_offset = file.seek(SeekFrom::End(0))?;
        file.write_all(&self.to_be_bytes())?;
        Ok(current_eof_offset)
    }

    /// Reads a [`RecordHeader`] from the current position of the given reader.
    ///
    /// Consumes exactly [`RECORD_HEADER_LEN`] bytes and leaves the reader
    /// positioned at the first byte of the key payload.
    pub fn read_header(file: &mut impl Read) -> io::Result<RecordHeader> {
        let mut c32_buf = [0u8; CRC_LEN];
        let mut k_buf = [0u8; SIZE_FIELD_LEN];
        let mut v_buf = [0u8; SIZE_FIELD_LEN];
        let mut t_buf = [0u8; TOMBSTONE_LEN];
        file.read_exact(&mut c32_buf)?;
        file.read_exact(&mut k_buf)?;
        file.read_exact(&mut v_buf)?;
        file.read_exact(&mut t_buf)?;
        Ok(RecordHeader {
            crc32: u32::from_be_bytes(c32_buf),
            key_size: u64::from_be_bytes(k_buf),
            value_size: u64::from_be_bytes(v_buf),
            tombstone: t_buf[0] != 0,
        })
    }

    /// Seeks to `offset` and reads a full record (header + key + value).
    pub fn read_record_at(file: &mut (impl Read + Seek), offset: u64) -> io::Result<Record> {
        file.seek(SeekFrom::Start(offset))?;
        Record::read_next(file)
    }

    /// Reads the next record from the current position of the reader.
    ///
    /// Parses the header, reads the key and value bytes, then verifies the
    /// CRC32 checksum. Returns [`ErrorKind::InvalidData`] on CRC mismatch
    /// or invalid UTF-8.
    pub fn read_next(file: &mut impl Read) -> io::Result<Record> {
        let header = Record::read_header(file)?;

        if header.key_size as usize > MAX_KEY_SIZE || header.value_size as usize > MAX_VALUE_SIZE {
            return Err(Error::new(
                std::io::ErrorKind::InvalidInput,
                "key or value exceeds maximum allowed size",
            ));
        }

        let mut k_buf = vec![0u8; header.key_size as usize];
        let mut v_buf: Vec<u8> = vec![0u8; header.value_size as usize];
        file.read_exact(&mut k_buf)?;
        file.read_exact(&mut v_buf)?;

        let mut payload = Vec::with_capacity(header.key_size as usize + header.value_size as usize);
        payload.extend_from_slice(&header.key_size.to_be_bytes());
        payload.extend_from_slice(&header.value_size.to_be_bytes());
        payload.extend_from_slice(&[header.tombstone as u8]);
        payload.extend_from_slice(&k_buf);
        payload.extend_from_slice(&v_buf);
        let crc32 = crc32(&payload);

        if crc32 != header.crc32 {
            return Err(Error::new(std::io::ErrorKind::InvalidData, "CRC mismatch"));
        }

        Ok(Record {
            header,
            key: String::from_utf8(k_buf).map_err(|e| Error::new(ErrorKind::InvalidData, e))?,
            value: String::from_utf8(v_buf).map_err(|e| Error::new(ErrorKind::InvalidData, e))?,
        })
    }

    pub fn to_be_bytes(&self) -> Vec<u8> {
        let total =
            RECORD_HEADER_LEN + self.header.key_size as usize + self.header.value_size as usize;
        let mut buf = Vec::with_capacity(total);
        buf.extend_from_slice(&[0u8; CRC_LEN]); // placeholder for CRC
        let payload_start = CRC_LEN;
        buf.extend_from_slice(&self.header.key_size.to_be_bytes());
        buf.extend_from_slice(&self.header.value_size.to_be_bytes());
        buf.extend_from_slice(&[self.header.tombstone as u8]);
        buf.extend_from_slice(self.key.as_bytes());
        buf.extend_from_slice(self.value.as_bytes());
        let checksum = crc32(&buf[payload_start..]);
        buf[..CRC_LEN].copy_from_slice(&checksum.to_be_bytes());
        buf
    }
}
