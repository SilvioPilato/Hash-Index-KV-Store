use std::{
    fs::{File, OpenOptions},
    io::{self, Error, ErrorKind, Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

use crate::record::{SIZE_FIELD_LEN, TOMBSTONE_LEN};

pub struct Hint {}
#[derive(Debug)]
pub struct HintEntry {
    pub key_size: u64,
    pub offset: u64,
    pub tombstone: bool,
    pub key: String,
}

impl Hint {
    pub fn write_file(path: PathBuf, entries: &[HintEntry]) -> io::Result<()> {
        let mut file: File = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        for entry in entries.iter() {
            let mut buf: Vec<u8> = Vec::with_capacity(
                SIZE_FIELD_LEN
                    + entry.offset.to_be_bytes().len()
                    + TOMBSTONE_LEN
                    + entry.key_size as usize,
            );

            buf.extend_from_slice(&entry.key_size.to_be_bytes());
            buf.extend_from_slice(&entry.offset.to_be_bytes());
            buf.extend_from_slice(&[entry.tombstone as u8]);
            buf.extend_from_slice(entry.key.as_bytes());

            file.write_all(&buf)?
        }

        Ok(())
    }

    pub fn read_file(path: PathBuf) -> io::Result<Vec<HintEntry>> {
        let mut file: File = OpenOptions::new().read(true).open(path)?;
        let mut entries: Vec<HintEntry> = Vec::new();

        let file_size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;
        while file.stream_position()? < file_size {
            let mut ks_buf = [0u8; SIZE_FIELD_LEN];
            let mut o_buf = [0u8; SIZE_FIELD_LEN];
            let mut t_buf = [0u8; TOMBSTONE_LEN];
            file.read_exact(&mut ks_buf)?;
            file.read_exact(&mut o_buf)?;
            file.read_exact(&mut t_buf)?;

            let key_size = u64::from_be_bytes(ks_buf);
            let offset = u64::from_be_bytes(o_buf);
            let tombstone = t_buf[0] != 0;

            let mut key_buf = vec![0u8; key_size as usize];
            file.read_exact(&mut key_buf)?;
            let key =
                String::from_utf8(key_buf).map_err(|e| Error::new(ErrorKind::InvalidData, e))?;

            entries.push(HintEntry {
                key_size,
                offset,
                tombstone,
                key,
            });
        }

        Ok(entries)
    }
}
