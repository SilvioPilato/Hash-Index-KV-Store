use hash_index::db::DB;
use hash_index::hint::{Hint, HintEntry};
use hash_index::settings::FSyncStrategy;
use std::fs;
use std::path::PathBuf;
use std::{env, time::SystemTime};

const DEFAULT_MAX_SEGMENT_BYTES: u64 = 1_048_576 * 50;

fn temp_db_path(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = env::temp_dir();
    path.push(format!("kv_store_hint_{}_{}", suffix, nanos));
    path.to_string_lossy().to_string()
}

fn hint_files_in(dir: &str) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().to_string();
            if name.ends_with(".hint") {
                Some(name)
            } else {
                None
            }
        })
        .collect()
}

// ── Hint entry round-trip (pure serialization, no DB) ──

#[test]
fn hint_entry_round_trip() {
    let entries = vec![
        HintEntry {
            key_size: 5,
            offset: 0,
            tombstone: false,
            key: "hello".to_string(),
        },
        HintEntry {
            key_size: 5,
            offset: 42,
            tombstone: false,
            key: "world".to_string(),
        },
        HintEntry {
            key_size: 7,
            offset: 0,
            tombstone: true,
            key: "deleted".to_string(),
        },
    ];

    let dir = temp_db_path("round_trip");
    fs::create_dir_all(&dir).unwrap();
    let path = PathBuf::from(&dir).join("test.hint");

    Hint::write_file(path.clone(), &entries).unwrap();
    let read_back = Hint::read_file(path).unwrap();

    assert_eq!(read_back.len(), 3);
    for (expected, actual) in entries.iter().zip(read_back.iter()) {
        assert_eq!(expected.key, actual.key);
        assert_eq!(expected.offset, actual.offset);
        assert_eq!(expected.tombstone, actual.tombstone);
        assert_eq!(expected.key_size, actual.key_size);
    }
}

#[test]
fn hint_read_empty_file() {
    let dir = temp_db_path("empty_hint");
    fs::create_dir_all(&dir).unwrap();
    let path = PathBuf::from(&dir).join("empty.hint");

    Hint::write_file(path.clone(), &[]).unwrap();
    let entries = Hint::read_file(path).unwrap();
    assert!(entries.is_empty());
}

// ── DB integration tests (require hint wiring in db.rs) ──

#[test]
fn compaction_produces_hint_files() {
    let path = temp_db_path("produces_hint");
    let mut db = DB::new(
        &path,
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    );
    db.set("k1", "v1").unwrap();
    db.set("k2", "v2").unwrap();

    // Before compaction — no hint files (active segment never gets one)
    assert!(hint_files_in(&path).is_empty());

    let _compacted = db.get_compacted().unwrap();

    // After compaction — at least one .hint file should exist
    let hints = hint_files_in(&path);
    assert!(!hints.is_empty(), "compaction should produce a .hint file");
}

#[test]
fn from_dir_loads_via_hint_files() {
    let path = temp_db_path("load_hint");
    {
        let mut db = DB::new(
            &path,
            "test",
            DEFAULT_MAX_SEGMENT_BYTES,
            FSyncStrategy::Always,
        );
        db.set("k1", "v1").unwrap();
        db.set("k2", "v2").unwrap();
        db.set("k1", "updated").unwrap();
        let _compacted = db.get_compacted().unwrap();
    }

    // Hint files exist from compaction
    assert!(!hint_files_in(&path).is_empty());

    // Reopen — should load index from hint files
    let db = DB::from_dir(
        &path,
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    )
    .unwrap()
    .unwrap();
    let (_, v1) = db.get("k1").unwrap().unwrap();
    let (_, v2) = db.get("k2").unwrap().unwrap();
    assert_eq!(v1, "updated");
    assert_eq!(v2, "v2");
}

#[test]
fn from_dir_falls_back_without_hint_files() {
    let path = temp_db_path("no_hint");
    {
        let mut db = DB::new(
            &path,
            "test",
            DEFAULT_MAX_SEGMENT_BYTES,
            FSyncStrategy::Always,
        );
        db.set("k1", "v1").unwrap();
        db.set("k2", "v2").unwrap();
        let _compacted = db.get_compacted().unwrap();
    }

    // Delete all hint files to force fallback to full scan
    for hint in hint_files_in(&path) {
        fs::remove_file(PathBuf::from(&path).join(hint)).unwrap();
    }
    assert!(hint_files_in(&path).is_empty());

    // Should still load correctly via full record scan
    let db = DB::from_dir(
        &path,
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    )
    .unwrap()
    .unwrap();
    let (_, v1) = db.get("k1").unwrap().unwrap();
    let (_, v2) = db.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

#[test]
fn compaction_cleans_up_old_hint_files() {
    let path = temp_db_path("cleanup_hint");
    let mut db = DB::new(
        &path,
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    );
    db.set("k1", "v1").unwrap();

    // First compaction — produces hint file(s)
    let mut compacted = db.get_compacted().unwrap();
    let hints_after_first = hint_files_in(&path);
    assert!(!hints_after_first.is_empty());

    // Add more data, compact again
    compacted.set("k2", "v2").unwrap();
    let compacted2 = compacted.get_compacted().unwrap();

    // The old hint files should be gone, replaced by new one(s)
    let hints_after_second = hint_files_in(&path);
    for old_hint in &hints_after_first {
        assert!(
            !hints_after_second.contains(old_hint),
            "old hint file {old_hint} should have been removed"
        );
    }

    // Data is still intact
    let (_, v1) = compacted2.get("k1").unwrap().unwrap();
    let (_, v2) = compacted2.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

#[test]
fn hint_files_with_multi_segment_compaction() {
    // Use tiny segment limit to force multiple segments, then compact
    let path = temp_db_path("multi_seg_hint");
    let mut db = DB::new(&path, "test", 50, FSyncStrategy::Always);
    db.set("k1", "value_one").unwrap();
    db.set("k2", "value_two").unwrap();
    db.set("k3", "value_three").unwrap();

    let _compacted = db.get_compacted().unwrap();

    // Hint files should exist
    assert!(!hint_files_in(&path).is_empty());

    // Reopen from disk — loads via hints
    let db2 = DB::from_dir(&path, "test", 50, FSyncStrategy::Always)
        .unwrap()
        .unwrap();
    let (_, v1) = db2.get("k1").unwrap().unwrap();
    let (_, v2) = db2.get("k2").unwrap().unwrap();
    let (_, v3) = db2.get("k3").unwrap().unwrap();
    assert_eq!(v1, "value_one");
    assert_eq!(v2, "value_two");
    assert_eq!(v3, "value_three");
}
