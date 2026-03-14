use std::{
    fs::File,
    io::{self, Error, ErrorKind, Read, Seek, SeekFrom, Write},
    vec,
};

use crate::crc::crc32;

pub const SIZE_FIELD_LEN: usize = 8;
const TOMBSTONE_LEN: usize = 1;
pub const RECORD_HEADER_LEN: usize = CRC_LEN + SIZE_FIELD_LEN * 2 + TOMBSTONE_LEN;
pub const MAX_KEY_SIZE: usize = 1_048_576;
pub const MAX_VALUE_SIZE: usize = 1_048_576 * 5;
pub const CRC_LEN: usize = 4;
/// The fixed-size header that precedes every key-value record on disk.
///
/// Layout: `|key_size (8 bytes BE u64)|value_size (8 bytes BE u64)|`
#[derive(Debug)]
pub struct RecordHeader {
    pub crc32: u32,
    /// Length of the key in bytes.
    pub key_size: u64,
    /// Length of the value in bytes.
    pub value_size: u64,
    pub tombstone: bool,
}

/// A complete key-value record: the fixed-size header followed by the key and value payloads.
#[derive(Debug)]
pub struct Record {
    pub header: RecordHeader,
    pub key: String,
    pub value: String,
}

impl Record {
    pub fn size_on_disk(&self) -> u64 {
        RECORD_HEADER_LEN as u64 + self.header.key_size + self.header.value_size
    }

    pub fn append(&self, file: &mut File) -> io::Result<u64> {
        let current_eof_offset = file.seek(SeekFrom::End(0))?;
        let mut buf = Vec::with_capacity(
            RECORD_HEADER_LEN + self.header.key_size as usize + self.header.value_size as usize,
        );
        let mut payload = Vec::with_capacity(
            RECORD_HEADER_LEN - CRC_LEN
                + self.header.key_size as usize
                + self.header.value_size as usize,
        );
        payload.extend_from_slice(&self.header.key_size.to_be_bytes());
        payload.extend_from_slice(&self.header.value_size.to_be_bytes());
        payload.extend_from_slice(&[self.header.tombstone as u8]);
        payload.extend_from_slice(self.key.as_bytes());
        payload.extend_from_slice(self.value.as_bytes());

        buf.extend_from_slice(&crc32(&payload).to_be_bytes());
        buf.extend_from_slice(&payload);

        file.write_all(&buf)?;
        Ok(current_eof_offset)
    }

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

    pub fn read_record_at(file: &mut (impl Read + Seek), offset: u64) -> io::Result<Record> {
        file.seek(SeekFrom::Start(offset))?;
        Record::read_next(file)
    }

    pub fn read_next(file: &mut impl Read) -> io::Result<Record> {
        match Record::read_header(file) {
            Ok(header) => {
                if header.key_size as usize > MAX_KEY_SIZE
                    || header.value_size as usize > MAX_VALUE_SIZE
                {
                    return Err(Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "key or value exceeds maximum allowed size",
                    ));
                }

                let mut k_buf = vec![0u8; header.key_size as usize];
                let mut v_buf: Vec<u8> = vec![0u8; header.value_size as usize];
                file.read_exact(&mut k_buf)?;
                file.read_exact(&mut v_buf)?;

                let mut payload =
                    Vec::with_capacity(header.key_size as usize + header.value_size as usize);
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
                    key: String::from_utf8(k_buf)
                        .map_err(|e| Error::new(ErrorKind::InvalidData, e))?,
                    value: String::from_utf8(v_buf)
                        .map_err(|e| Error::new(ErrorKind::InvalidData, e))?,
                })
            }
            Err(err) => Err(err),
        }
    }
}
