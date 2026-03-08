use std::{
    format,
    io::{self, Read},
    time::SystemTime,
};

/// Generates a unique database file name by appending the current Unix
/// timestamp (in seconds) to the given base path.
///
/// Returns a path like `"{db_file_path}_{epoch_secs}.db"`.
pub fn get_new_db_file_name(db_file_path: &str) -> Result<String, std::time::SystemTimeError> {
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(n) => Ok(format!("{}_{}.db", db_file_path, n.as_secs())),
        Err(_) => panic!("SystemTime before UNIX EPOCH!"),
    }
}

/// The fixed-size header that precedes every key-value record on disk.
///
/// Layout: `|key_size (8 bytes BE u64)|value_size (8 bytes BE u64)|`
pub struct RecordHeader {
    /// Length of the key in bytes.
    pub key_size: u64,
    /// Length of the value in bytes.
    pub value_size: u64,
}

/// Reads a [`RecordHeader`] from the current position of the given reader.
///
/// Consumes exactly 16 bytes (two big-endian `u64` values) and returns
/// the parsed key and value sizes. The reader is left positioned at the
/// first byte of the key payload.
pub fn read_record_header(file: &mut impl Read) -> io::Result<RecordHeader> {
    let mut k_buf = [0u8; 8];
    let mut v_buf = [0u8; 8];
    file.read_exact(&mut k_buf)?;
    file.read_exact(&mut v_buf)?;
    Ok(RecordHeader {
        key_size: u64::from_be_bytes(k_buf),
        value_size: u64::from_be_bytes(v_buf),
    })
}
