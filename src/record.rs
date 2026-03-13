use std::{
    fs::File,
    io::{self, Error, ErrorKind, Read, Seek, SeekFrom, Write},
    vec,
};

pub const SIZE_FIELD_LEN: usize = 8;
const TOMBSTONE_LEN: usize = 1;
pub const RECORD_HEADER_LEN: usize = SIZE_FIELD_LEN * 2 + TOMBSTONE_LEN;
pub const MAX_KEY_SIZE: usize = 1_048_576;
pub const MAX_VALUE_SIZE: usize = 1_048_576 * 5;
/// The fixed-size header that precedes every key-value record on disk.
///
/// Layout: `|key_size (8 bytes BE u64)|value_size (8 bytes BE u64)|`
pub struct RecordHeader {
    /// Length of the key in bytes.
    pub key_size: u64,
    /// Length of the value in bytes.
    pub value_size: u64,

    pub tombstone: bool,
}

/// A complete key-value record: the fixed-size header followed by the key and value payloads.
pub struct Record {
    pub header: RecordHeader,
    pub key: String,
    pub value: String,
}

/// Reads a [`RecordHeader`] from the current position of the given reader.
///
/// Consumes exactly 16 bytes (two big-endian `u64` values) and returns
/// the parsed key and value sizes. The reader is left positioned at the
/// first byte of the key payload.
pub fn read_record_header(file: &mut impl Read) -> io::Result<RecordHeader> {
    let mut k_buf = [0u8; SIZE_FIELD_LEN];
    let mut v_buf = [0u8; SIZE_FIELD_LEN];
    let mut t_buf = [0u8; TOMBSTONE_LEN];

    file.read_exact(&mut k_buf)?;
    file.read_exact(&mut v_buf)?;
    file.read_exact(&mut t_buf)?;
    Ok(RecordHeader {
        key_size: u64::from_be_bytes(k_buf),
        value_size: u64::from_be_bytes(v_buf),
        tombstone: t_buf[0] != 0,
    })
}

pub fn read_record(file: &mut impl Read) -> io::Result<Record> {
    match read_record_header(file) {
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

            Ok(Record {
                header,
                key: String::from_utf8(k_buf).map_err(|e| Error::new(ErrorKind::InvalidData, e))?,
                value: String::from_utf8(v_buf)
                    .map_err(|e| Error::new(ErrorKind::InvalidData, e))?,
            })
        }
        Err(err) => Err(err),
    }
}

pub fn read_record_at(file: &mut (impl Read + Seek), offset: u64) -> io::Result<Record> {
    file.seek(SeekFrom::Start(offset))?;
    read_record(file)
}

pub fn append_record(file: &mut File, record: &Record) -> io::Result<u64> {
    let current_eof_offset = file.seek(SeekFrom::End(0))?;

    let mut buf = Vec::with_capacity(
        RECORD_HEADER_LEN + record.header.key_size as usize + record.header.value_size as usize,
    );
    buf.extend_from_slice(&record.header.key_size.to_be_bytes());
    buf.extend_from_slice(&record.header.value_size.to_be_bytes());
    buf.extend_from_slice(&[record.header.tombstone as u8]);
    buf.extend_from_slice(record.key.as_bytes());
    buf.extend_from_slice(record.value.as_bytes());
    file.write_all(&buf)?;
    file.sync_all()?;
    Ok(current_eof_offset)
}
