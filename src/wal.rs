use std::{
    fs::{self, File, OpenOptions},
    io::{self, Seek, SeekFrom},
    path::PathBuf,
};

use crate::{
    memtable::Memtable,
    record::{Record, RecordHeader},
};

pub struct Wal {
    file: File,
    path: PathBuf,
}

impl Wal {
    pub fn open(path: &PathBuf, name: String) -> io::Result<Wal> {
        let filename = format!("{}.wal", name);
        let file_path = PathBuf::from(path).join(filename);
        let file: File = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&file_path)?;

        Ok(Wal {
            file,
            path: file_path.clone(),
        })
    }

    pub fn append(&mut self, key: String, value: String, tombstone: bool) -> io::Result<u64> {
        let record = Record {
            header: RecordHeader {
                crc32: 0,
                key_size: key.len() as u64,
                value_size: value.len() as u64,
                tombstone,
            },
            key,
            value,
        };
        record.append(&mut self.file)
    }

    pub fn replay(&self) -> io::Result<Memtable> {
        let mut file = self.file.try_clone()?;
        let file_size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;
        let mut memtable = Memtable::new();
        while file.stream_position()? < file_size {
            let record = match Record::read_next(&mut file) {
                Ok(record) => record,
                Err(e)
                    if e.kind() == io::ErrorKind::UnexpectedEof
                        || e.kind() == io::ErrorKind::InvalidData =>
                {
                    break;
                }
                Err(e) => return Err(e),
            };

            if record.header.tombstone {
                memtable.remove(record.key);
                continue;
            }

            let key = record.key;
            let value = record.value;

            memtable.insert(key, value);
        }

        Ok(memtable)
    }

    pub fn reset(&mut self) -> io::Result<()> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        Ok(())
    }

    pub fn delete_file(self) -> io::Result<()> {
        drop(self.file);
        fs::remove_file(&self.path)
    }
}
