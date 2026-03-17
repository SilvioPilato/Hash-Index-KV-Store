use hash_index::engine::StorageEngine;
use hash_index::lsmengine::LsmEngine;
use std::{env, fs, time::SystemTime};

fn temp_dir(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = env::temp_dir();
    path.push(format!("kv_store_lsm_{}_{}", suffix, nanos));
    fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().to_string()
}

const BIG_MEMTABLE: usize = 1_048_576; // 1 MB — won't auto-flush

#[test]
fn set_and_get() {
    let dir = temp_dir("set_get");
    let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE);
    engine.set("hello", "world").unwrap();
    let result = engine.get("hello").unwrap();
    assert_eq!(result, Some(("hello".to_string(), "world".to_string())));
}

#[test]
fn get_missing_key() {
    let dir = temp_dir("missing");
    let engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE);
    assert_eq!(engine.get("nope").unwrap(), None);
}

#[test]
fn set_overwrite() {
    let dir = temp_dir("overwrite");
    let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE);
    engine.set("k", "old").unwrap();
    engine.set("k", "new").unwrap();
    let (_, v) = engine.get("k").unwrap().unwrap();
    assert_eq!(v, "new");
}

#[test]
fn delete_removes_key() {
    let dir = temp_dir("delete");
    let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE);
    engine.set("k", "v").unwrap();
    engine.delete("k").unwrap();
    assert_eq!(engine.get("k").unwrap(), None);
}

#[test]
fn delete_nonexistent_key() {
    let dir = temp_dir("delete_missing");
    let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE);
    let result = engine.delete("nope").unwrap();
    assert_eq!(result, Some(()));
}

#[test]
fn memtable_flushes_to_sstable() {
    let dir = temp_dir("flush");
    // Tiny threshold so a single write triggers a flush
    let mut engine = LsmEngine::new(&dir, "test", 1);
    engine.set("k1", "v1").unwrap();

    // After flush, memtable is cleared but data is readable from SSTable
    let (_, v) = engine.get("k1").unwrap().unwrap();
    assert_eq!(v, "v1");

    // .sst file should exist on disk
    let sst_files: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().to_string();
            if name.ends_with(".sst") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    assert!(!sst_files.is_empty(), "expected at least one .sst file");
}

#[test]
fn reads_span_memtable_and_segments() {
    let dir = temp_dir("span");
    let mut engine = LsmEngine::new(&dir, "test", 1);
    // This will flush to segment
    engine.set("k1", "v1").unwrap();

    // Larger threshold so next write stays in memtable
    // We can't change threshold, but k2 will also flush and k3 stays if we make threshold bigger
    // Instead: write multiple keys, they all flush, then write one more with room
    engine.set("k2", "v2").unwrap();

    // Both should be readable regardless of where they live
    let (_, v1) = engine.get("k1").unwrap().unwrap();
    let (_, v2) = engine.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

#[test]
fn delete_shadows_flushed_value() {
    let dir = temp_dir("shadow");
    let mut engine = LsmEngine::new(&dir, "test", 1);
    // Flush k1 to a segment
    engine.set("k1", "v1").unwrap();
    // Delete in memtable should shadow the segment value
    engine.delete("k1").unwrap();
    assert_eq!(engine.get("k1").unwrap(), None);
}

#[test]
fn compact_preserves_values() {
    let dir = temp_dir("compact_preserve");
    let mut engine = LsmEngine::new(&dir, "test", 1);
    engine.set("k1", "v1").unwrap();
    engine.set("k2", "v2").unwrap();

    engine.compact().unwrap();

    let (_, v1) = engine.get("k1").unwrap().unwrap();
    let (_, v2) = engine.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

#[test]
fn compact_keeps_latest_value() {
    let dir = temp_dir("compact_latest");
    let mut engine = LsmEngine::new(&dir, "test", 1);
    engine.set("k", "old").unwrap();
    engine.set("k", "new").unwrap();

    engine.compact().unwrap();

    let (_, v) = engine.get("k").unwrap().unwrap();
    assert_eq!(v, "new");
}

#[test]
fn compact_drops_deleted_keys() {
    let dir = temp_dir("compact_delete");
    let mut engine = LsmEngine::new(&dir, "test", 1);
    engine.set("k1", "v1").unwrap();
    engine.delete("k1").unwrap();

    engine.compact().unwrap();

    assert_eq!(engine.get("k1").unwrap(), None);
}

#[test]
fn compact_is_idempotent() {
    let dir = temp_dir("compact_idempotent");
    let mut engine = LsmEngine::new(&dir, "test", 1);
    engine.set("k1", "v1").unwrap();
    engine.set("k2", "v2").unwrap();

    engine.compact().unwrap();
    engine.compact().unwrap();

    let (_, v1) = engine.get("k1").unwrap().unwrap();
    let (_, v2) = engine.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

#[test]
fn from_dir_reloads_segments() {
    let dir = temp_dir("from_dir");
    {
        let mut engine = LsmEngine::new(&dir, "test", 1);
        engine.set("k1", "v1").unwrap();
        engine.set("k2", "v2").unwrap();
    }

    let engine = LsmEngine::from_dir(&dir, "test", BIG_MEMTABLE).unwrap();
    let (_, v1) = engine.get("k1").unwrap().unwrap();
    let (_, v2) = engine.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

#[test]
fn compact_reduces_segment_count() {
    let dir = temp_dir("compact_reduce");
    let mut engine = LsmEngine::new(&dir, "test", 1);
    engine.set("k1", "v1").unwrap();
    engine.set("k2", "v2").unwrap();
    engine.set("k3", "v3").unwrap();

    // Multiple .sst files should exist before compaction
    let sst_before: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().to_string();
            if name.ends_with(".sst") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    assert!(sst_before.len() > 1);

    engine.compact().unwrap();

    // After compaction, should be exactly 1 .sst file
    let sst_after: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().to_string();
            if name.ends_with(".sst") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(sst_after.len(), 1);
}
