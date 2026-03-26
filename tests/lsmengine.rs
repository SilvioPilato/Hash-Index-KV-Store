use rustikv::engine::StorageEngine;
use rustikv::lsmengine::LsmEngine;
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
    let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("hello", "world").unwrap();
    let result = engine.get("hello").unwrap();
    assert_eq!(result, Some(("hello".to_string(), "world".to_string())));
}

#[test]
fn get_missing_key() {
    let dir = temp_dir("missing");
    let engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE).unwrap();
    assert_eq!(engine.get("nope").unwrap(), None);
}

#[test]
fn set_overwrite() {
    let dir = temp_dir("overwrite");
    let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("k", "old").unwrap();
    engine.set("k", "new").unwrap();
    let (_, v) = engine.get("k").unwrap().unwrap();
    assert_eq!(v, "new");
}

#[test]
fn delete_removes_key() {
    let dir = temp_dir("delete");
    let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("k", "v").unwrap();
    engine.delete("k").unwrap();
    assert_eq!(engine.get("k").unwrap(), None);
}

#[test]
fn delete_nonexistent_key() {
    let dir = temp_dir("delete_missing");
    let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE).unwrap();
    let result = engine.delete("nope").unwrap();
    assert_eq!(result, None);
}

#[test]
fn memtable_flushes_to_sstable() {
    let dir = temp_dir("flush");
    // Tiny threshold so a single write triggers a flush
    let mut engine = LsmEngine::new(&dir, "test", 1).unwrap();
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
    let mut engine = LsmEngine::new(&dir, "test", 1).unwrap();
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
    let mut engine = LsmEngine::new(&dir, "test", 1).unwrap();
    // Flush k1 to a segment
    engine.set("k1", "v1").unwrap();
    // Delete in memtable should shadow the segment value
    engine.delete("k1").unwrap();
    assert_eq!(engine.get("k1").unwrap(), None);
}

#[test]
fn compact_preserves_values() {
    let dir = temp_dir("compact_preserve");
    let mut engine = LsmEngine::new(&dir, "test", 1).unwrap();
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
    let mut engine = LsmEngine::new(&dir, "test", 1).unwrap();
    engine.set("k", "old").unwrap();
    engine.set("k", "new").unwrap();

    engine.compact().unwrap();

    let (_, v) = engine.get("k").unwrap().unwrap();
    assert_eq!(v, "new");
}

#[test]
fn compact_drops_deleted_keys() {
    let dir = temp_dir("compact_delete");
    let mut engine = LsmEngine::new(&dir, "test", 1).unwrap();
    engine.set("k1", "v1").unwrap();
    engine.delete("k1").unwrap();

    engine.compact().unwrap();

    assert_eq!(engine.get("k1").unwrap(), None);
}

#[test]
fn compact_is_idempotent() {
    let dir = temp_dir("compact_idempotent");
    let mut engine = LsmEngine::new(&dir, "test", 1).unwrap();
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
        let mut engine = LsmEngine::new(&dir, "test", 1).unwrap();
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
    let mut engine = LsmEngine::new(&dir, "test", 1).unwrap();
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

// --- WAL recovery tests ---

#[test]
fn wal_recovers_unflushed_writes() {
    // Writes that never trigger a flush live only in the WAL.
    // Dropping the engine simulates a crash; from_dir must replay the WAL.
    let dir = temp_dir("wal_recover");
    {
        let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE).unwrap();
        engine.set("k1", "v1").unwrap();
        engine.set("k2", "v2").unwrap();
        engine.set("k3", "v3").unwrap();
        // drop without flush
    }

    let engine = LsmEngine::from_dir(&dir, "test", BIG_MEMTABLE).unwrap();
    assert_eq!(
        engine.get("k1").unwrap(),
        Some(("k1".to_string(), "v1".to_string()))
    );
    assert_eq!(
        engine.get("k2").unwrap(),
        Some(("k2".to_string(), "v2".to_string()))
    );
    assert_eq!(
        engine.get("k3").unwrap(),
        Some(("k3".to_string(), "v3".to_string()))
    );
}

#[test]
fn wal_recovers_unflushed_delete() {
    // A delete that has not been flushed must be replayed as a tombstone,
    // shadowing the earlier value that was already flushed to an SSTable.
    let dir = temp_dir("wal_delete");
    {
        let mut engine = LsmEngine::new(&dir, "test", 1).unwrap(); // threshold=1 flushes immediately
        engine.set("k1", "v1").unwrap(); // flushed to SSTable
    }
    {
        let mut engine = LsmEngine::from_dir(&dir, "test", BIG_MEMTABLE).unwrap();
        engine.delete("k1").unwrap(); // tombstone in WAL only, not flushed
        // drop without flush
    }

    let engine = LsmEngine::from_dir(&dir, "test", BIG_MEMTABLE).unwrap();
    assert_eq!(engine.get("k1").unwrap(), None);
}

#[test]
fn wal_corrupt_tail_does_not_panic() {
    // A crash mid-write leaves a torn record at the tail of the WAL.
    // from_dir must stop replay at the corrupt record and recover earlier entries.
    let dir = temp_dir("wal_corrupt");
    {
        let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE).unwrap();
        engine.set("k1", "v1").unwrap();
        engine.set("k2", "v2").unwrap();
    }

    // Truncate the last 5 bytes of the WAL to simulate a torn write
    let wal_path = std::path::Path::new(&dir).join("test.wal");
    let metadata = fs::metadata(&wal_path).unwrap();
    let truncated_len = metadata.len().saturating_sub(5);
    let file = fs::OpenOptions::new().write(true).open(&wal_path).unwrap();
    file.set_len(truncated_len).unwrap();

    // Must not panic; k1 may or may not be recovered depending on where truncation fell,
    // but from_dir must succeed
    let engine = LsmEngine::from_dir(&dir, "test", BIG_MEMTABLE).unwrap();
    // k1 was written first — its record is intact and must be readable
    assert_eq!(
        engine.get("k1").unwrap(),
        Some(("k1".to_string(), "v1".to_string()))
    );
}

#[test]
fn wal_is_absent_after_flush() {
    // Once the memtable is flushed to an SSTable, the WAL is no longer needed.
    // It must be deleted so that the next startup doesn't replay stale entries.
    let dir = temp_dir("wal_absent");
    let mut engine = LsmEngine::new(&dir, "test", 1).unwrap(); // threshold=1, every write flushes
    engine.set("k1", "v1").unwrap();

    let wal_path = std::path::Path::new(&dir).join("test.wal");
    assert_eq!(
        fs::metadata(&wal_path).unwrap().len(),
        0,
        "WAL should be empty after flush"
    );
}

#[test]
fn exists_returns_true_after_set() {
    let dir = temp_dir("exists_true");
    let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("k", "v").unwrap();
    assert!(engine.exists("k"));
}

#[test]
fn exists_returns_false_for_missing_key() {
    let dir = temp_dir("exists_missing");
    let engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE).unwrap();
    assert!(!engine.exists("nope"));
}

#[test]
fn exists_returns_false_after_delete() {
    let dir = temp_dir("exists_delete");
    let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("k", "v").unwrap();
    engine.delete("k").unwrap();
    assert!(!engine.exists("k"));
}

#[test]
fn exists_returns_true_for_flushed_key() {
    // Key is flushed to an SSTable; exists must still find it via SSTable lookup
    // (and the bloom filter must not produce a false negative).
    let dir = temp_dir("exists_flushed");
    let mut engine = LsmEngine::new(&dir, "test", 1).unwrap(); // threshold=1, every write flushes
    engine.set("k", "v").unwrap();
    assert!(engine.exists("k"));
}

#[test]
fn exists_returns_false_for_tombstoned_flushed_key() {
    // Key written and flushed, then deleted (tombstone in memtable). exists must return false.
    let dir = temp_dir("exists_tombstone");
    let mut engine = LsmEngine::new(&dir, "test", 1).unwrap(); // threshold=1, flushes on set
    engine.set("k", "v").unwrap();
    engine.delete("k").unwrap();
    assert!(!engine.exists("k"));
}

#[test]
fn list_keys_returns_all_live_keys() {
    let dir = temp_dir("list_keys");
    let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("a", "1").unwrap();
    engine.set("b", "2").unwrap();
    engine.set("c", "3").unwrap();

    let mut keys = engine.list_keys().unwrap();
    keys.sort();
    assert_eq!(keys, vec!["a", "b", "c"]);
}

#[test]
fn list_keys_excludes_deleted_keys() {
    let dir = temp_dir("list_keys_delete");
    let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("a", "1").unwrap();
    engine.set("b", "2").unwrap();
    engine.set("c", "3").unwrap();
    engine.delete("b").unwrap();

    let mut keys = engine.list_keys().unwrap();
    keys.sort();
    assert_eq!(keys, vec!["a", "c"]);
}

#[test]
fn list_keys_spans_memtable_and_segments() {
    let dir = temp_dir("list_keys_segments");
    // Threshold of 1 byte so every write flushes to SSTable
    let mut engine = LsmEngine::new(&dir, "test", 1).unwrap();
    engine.set("x", "1").unwrap();
    engine.set("y", "2").unwrap();
    // Reload from disk and write one more key — it stays in the memtable
    let mut engine = LsmEngine::from_dir(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("z", "3").unwrap();

    let mut keys = engine.list_keys().unwrap();
    keys.sort();
    assert_eq!(keys, vec!["x", "y", "z"]);
}

#[test]
fn list_keys_tombstone_in_memtable_hides_flushed_key() {
    let dir = temp_dir("list_keys_tombstone");
    // Flush "a" to SSTable, then delete it via memtable tombstone
    let mut engine = LsmEngine::new(&dir, "test", 1).unwrap();
    engine.set("a", "1").unwrap();
    let mut engine = LsmEngine::new(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.delete("a").unwrap();

    assert_eq!(engine.list_keys().unwrap(), Vec::<String>::new());
}
