use rustikv::crc::crc32;
use rustikv::record::{Record, RecordHeader};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};

fn temp_file(name: &str) -> (File, String) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir()
        .join(format!("crc_test_{}_{}", name, nanos))
        .to_string_lossy()
        .to_string();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    (file, path)
}

#[test]
fn known_test_vector() {
    // Standard CRC32 check value from the IEEE spec
    assert_eq!(crc32(b"123456789"), 0xCBF43926);
}

#[test]
fn valid_record_roundtrip() {
    let (mut file, _path) = temp_file("roundtrip");
    let record = Record {
        header: RecordHeader {
            crc32: 0,
            key_size: 5,
            value_size: 5,
            tombstone: false,
        },
        key: "hello".to_string(),
        value: "world".to_string(),
    };
    record.append(&mut file).unwrap();

    file.seek(SeekFrom::Start(0)).unwrap();
    let read_back = Record::read_next(&mut file).unwrap();
    assert_eq!(read_back.key, "hello");
    assert_eq!(read_back.value, "world");
}

#[test]
fn corrupted_value_detected() {
    let (mut file, path) = temp_file("corrupt_value");
    let record = Record {
        header: RecordHeader {
            crc32: 0,
            key_size: 5,
            value_size: 5,
            tombstone: false,
        },
        key: "hello".to_string(),
        value: "world".to_string(),
    };
    record.append(&mut file).unwrap();

    // Flip a byte in the value region (last byte of the file)
    let file_len = file.seek(SeekFrom::End(0)).unwrap();
    file.seek(SeekFrom::Start(file_len - 1)).unwrap();
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte).unwrap();
    byte[0] ^= 0xFF;
    file.seek(SeekFrom::Start(file_len - 1)).unwrap();
    file.write_all(&byte).unwrap();
    file.sync_all().unwrap();

    // Re-open and try to read — should fail with InvalidData
    let mut file = File::open(&path).unwrap();
    let err = Record::read_next(&mut file).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("CRC mismatch"));
}

#[test]
fn corrupted_header_detected() {
    let (mut file, path) = temp_file("corrupt_header");
    let record = Record {
        header: RecordHeader {
            crc32: 0,
            key_size: 5,
            value_size: 5,
            tombstone: false,
        },
        key: "hello".to_string(),
        value: "world".to_string(),
    };
    record.append(&mut file).unwrap();

    // Flip a byte in the tombstone field (byte at offset CRC_LEN + 16 = 20)
    let tombstone_offset = 20u64;
    file.seek(SeekFrom::Start(tombstone_offset)).unwrap();
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte).unwrap();
    byte[0] ^= 0xFF;
    file.seek(SeekFrom::Start(tombstone_offset)).unwrap();
    file.write_all(&byte).unwrap();
    file.sync_all().unwrap();

    let mut file = File::open(&path).unwrap();
    let err = Record::read_next(&mut file).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("CRC mismatch"));
}
