use rustikv::engine::{RangeScan, StorageEngine};
use rustikv::leveled::Leveled;
use rustikv::lsmengine::LsmEngine;
use rustikv::sstable::SSTable;
use std::{env, fs, time::SystemTime};

fn temp_dir(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = env::temp_dir();
    path.push(format!("kv_store_leveled_{}_{}", suffix, nanos));
    fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().to_string()
}

// 4 levels, L0 threshold = 4 files, L1 max = 10MB
fn new_engine(dir: &str, db_name: &str, max_memtable_bytes: usize) -> std::io::Result<LsmEngine> {
    let strategy = Box::new(Leveled::new(4, 4, 10 * 1024 * 1024));
    LsmEngine::new(dir, db_name, max_memtable_bytes, strategy)
}

fn engine_from_dir(
    dir: &str,
    db_name: &str,
    max_memtable_bytes: usize,
) -> std::io::Result<LsmEngine> {
    let strategy = Box::new(Leveled::load_from_dir(
        dir,
        db_name,
        4,
        4,
        10 * 1024 * 1024,
    )?);
    LsmEngine::from_dir(dir, db_name, max_memtable_bytes, strategy)
}

const BIG_MEMTABLE: usize = 1_048_576; // 1 MB — won't auto-flush

// --- SSTable level filename parsing ---

#[test]
fn parse_leveled_filename() {
    let sst = SSTable::parse("mydb_L2_1234567890.sst");
    assert!(sst.is_some());
    let sst = sst.unwrap();
    assert_eq!(sst.level, Some(2));
    assert_eq!(sst.timestamp, 1234567890);
}

#[test]
fn parse_non_leveled_filename() {
    let sst = SSTable::parse("mydb_1234567890.sst");
    assert!(sst.is_some());
    let sst = sst.unwrap();
    assert_eq!(sst.level, None);
    assert_eq!(sst.timestamp, 1234567890);
}

#[test]
fn parse_level_0_filename() {
    let sst = SSTable::parse("mydb_L0_9999.sst");
    assert!(sst.is_some());
    let sst = sst.unwrap();
    assert_eq!(sst.level, Some(0));
}

// --- Basic engine operations with Leveled strategy ---

#[test]
fn set_and_get() {
    let dir = temp_dir("set_get");
    let mut engine = new_engine(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("hello", "world").unwrap();
    let result = engine.get("hello").unwrap();
    assert_eq!(result, Some(("hello".to_string(), "world".to_string())));
}

#[test]
fn get_missing_key() {
    let dir = temp_dir("missing");
    let engine = new_engine(&dir, "test", BIG_MEMTABLE).unwrap();
    assert_eq!(engine.get("nope").unwrap(), None);
}

#[test]
fn set_overwrite() {
    let dir = temp_dir("overwrite");
    let mut engine = new_engine(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("k", "old").unwrap();
    engine.set("k", "new").unwrap();
    let (_, v) = engine.get("k").unwrap().unwrap();
    assert_eq!(v, "new");
}

#[test]
fn delete_removes_key() {
    let dir = temp_dir("delete");
    let mut engine = new_engine(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("k", "v").unwrap();
    engine.delete("k").unwrap();
    assert_eq!(engine.get("k").unwrap(), None);
}

#[test]
fn delete_nonexistent_key() {
    let dir = temp_dir("delete_missing");
    let mut engine = new_engine(&dir, "test", BIG_MEMTABLE).unwrap();
    let result = engine.delete("nope").unwrap();
    assert_eq!(result, None);
}

// --- Flush and segment tests ---

#[test]
fn memtable_flushes_to_sstable() {
    let dir = temp_dir("flush");
    {
        let engine = new_engine(&dir, "test", 1).unwrap();
        engine.set("k1", "v1").unwrap();
    } // drop joins the background flush

    let engine = engine_from_dir(&dir, "test", BIG_MEMTABLE).unwrap();

    let (_, v) = engine.get("k1").unwrap().unwrap();
    assert_eq!(v, "v1");

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
    let mut engine = new_engine(&dir, "test", 1).unwrap();
    engine.set("k1", "v1").unwrap();
    engine.set("k2", "v2").unwrap();

    let (_, v1) = engine.get("k1").unwrap().unwrap();
    let (_, v2) = engine.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

#[test]
fn delete_shadows_flushed_value() {
    let dir = temp_dir("shadow");
    let mut engine = new_engine(&dir, "test", 1).unwrap();
    engine.set("k1", "v1").unwrap();
    engine.delete("k1").unwrap();
    assert_eq!(engine.get("k1").unwrap(), None);
}

// --- Compaction tests ---

#[test]
fn compact_preserves_values() {
    let dir = temp_dir("compact_preserve");
    let mut engine = new_engine(&dir, "test", 1).unwrap();
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
    let mut engine = new_engine(&dir, "test", 1).unwrap();
    engine.set("k", "old").unwrap();
    engine.set("k", "new").unwrap();

    engine.compact().unwrap();

    let (_, v) = engine.get("k").unwrap().unwrap();
    assert_eq!(v, "new");
}

#[test]
fn compact_drops_deleted_keys() {
    let dir = temp_dir("compact_delete");
    let mut engine = new_engine(&dir, "test", 1).unwrap();
    engine.set("k1", "v1").unwrap();
    engine.delete("k1").unwrap();

    engine.compact().unwrap();

    assert_eq!(engine.get("k1").unwrap(), None);
}

#[test]
fn compact_is_idempotent() {
    let dir = temp_dir("compact_idempotent");
    let mut engine = new_engine(&dir, "test", 1).unwrap();
    engine.set("k1", "v1").unwrap();
    engine.set("k2", "v2").unwrap();

    engine.compact().unwrap();
    engine.compact().unwrap();

    let (_, v1) = engine.get("k1").unwrap().unwrap();
    let (_, v2) = engine.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

// --- Load from dir / level placement ---

#[test]
fn from_dir_reloads_segments() {
    let dir = temp_dir("from_dir");
    {
        let mut engine = new_engine(&dir, "test", 1).unwrap();
        engine.set("k1", "v1").unwrap();
        engine.set("k2", "v2").unwrap();
    }

    let engine = engine_from_dir(&dir, "test", BIG_MEMTABLE).unwrap();
    let (_, v1) = engine.get("k1").unwrap().unwrap();
    let (_, v2) = engine.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

#[test]
fn from_dir_preserves_level_in_filename() {
    let dir = temp_dir("level_filename");
    {
        let mut engine = new_engine(&dir, "test", 1).unwrap();
        engine.set("k1", "v1").unwrap();
        engine.set("k2", "v2").unwrap();
        engine.set("k3", "v3").unwrap();
        engine.set("k4", "v4").unwrap();
        // With threshold=1, each write flushes to L0.
        // After compaction, files should have level info in filename.
        engine.compact().unwrap();
    }

    let sst_files: Vec<String> = fs::read_dir(&dir)
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

    // After compact_all, files should have _L in the name
    assert!(
        sst_files.iter().any(|f| f.contains("_L")),
        "expected leveled filenames after compaction, got: {:?}",
        sst_files
    );
}

#[test]
fn from_dir_reloads_after_compaction() {
    let dir = temp_dir("reload_compact");
    {
        let mut engine = new_engine(&dir, "test", 1).unwrap();
        for i in 0..10 {
            engine
                .set(&format!("key{:03}", i), &format!("val{}", i))
                .unwrap();
        }
        engine.compact().unwrap();
    }

    // Reload and verify all data
    let engine = engine_from_dir(&dir, "test", BIG_MEMTABLE).unwrap();
    for i in 0..10 {
        let (_, v) = engine
            .get(&format!("key{:03}", i))
            .unwrap()
            .unwrap_or_else(|| panic!("key{:03} not found after reload", i));
        assert_eq!(v, format!("val{}", i));
    }
}

// --- Exists ---

#[test]
fn exists_returns_true_after_set() {
    let dir = temp_dir("exists_true");
    let mut engine = new_engine(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("k", "v").unwrap();
    assert!(engine.exists("k"));
}

#[test]
fn exists_returns_false_for_missing_key() {
    let dir = temp_dir("exists_missing");
    let engine = new_engine(&dir, "test", BIG_MEMTABLE).unwrap();
    assert!(!engine.exists("nope"));
}

#[test]
fn exists_returns_false_after_delete() {
    let dir = temp_dir("exists_delete");
    let mut engine = new_engine(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("k", "v").unwrap();
    engine.delete("k").unwrap();
    assert!(!engine.exists("k"));
}

#[test]
fn exists_returns_true_for_flushed_key() {
    let dir = temp_dir("exists_flushed");
    let mut engine = new_engine(&dir, "test", 1).unwrap();
    engine.set("k", "v").unwrap();
    assert!(engine.exists("k"));
}

// --- List keys ---

#[test]
fn list_keys_returns_all_live_keys() {
    let dir = temp_dir("list_keys");
    let mut engine = new_engine(&dir, "test", BIG_MEMTABLE).unwrap();
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
    let mut engine = new_engine(&dir, "test", BIG_MEMTABLE).unwrap();
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
    let engine = new_engine(&dir, "test", 1).unwrap();
    engine.set("x", "1").unwrap();
    engine.set("y", "2").unwrap();
    drop(engine);
    let engine = engine_from_dir(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("z", "3").unwrap();

    let mut keys = engine.list_keys().unwrap();
    keys.sort();
    assert_eq!(keys, vec!["x", "y", "z"]);
}

// --- Range scan ---

#[test]
fn range_basic_memtable() {
    let dir = temp_dir("range_basic");
    let mut engine = new_engine(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("a", "1").unwrap();
    engine.set("b", "2").unwrap();
    engine.set("c", "3").unwrap();
    engine.set("d", "4").unwrap();

    let results = engine.range("b", "c").unwrap();
    assert_eq!(
        results,
        vec![
            ("b".to_string(), "2".to_string()),
            ("c".to_string(), "3".to_string())
        ]
    );
}

#[test]
fn range_spans_memtable_and_segment() {
    let dir = temp_dir("range_span");
    let engine = new_engine(&dir, "test", 1).unwrap();
    engine.set("a", "1").unwrap();
    engine.set("c", "3").unwrap();
    drop(engine);

    let engine = engine_from_dir(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("b", "2").unwrap();

    let results = engine.range("a", "c").unwrap();
    assert_eq!(
        results,
        vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
            ("c".to_string(), "3".to_string()),
        ]
    );
}

#[test]
fn range_after_compaction() {
    let dir = temp_dir("range_compact");
    let mut engine = new_engine(&dir, "test", 1).unwrap();
    engine.set("a", "1").unwrap();
    engine.set("b", "2").unwrap();
    engine.set("c", "3").unwrap();
    engine.set("d", "4").unwrap();

    engine.compact().unwrap();

    let results = engine.range("b", "c").unwrap();
    assert_eq!(
        results,
        vec![
            ("b".to_string(), "2".to_string()),
            ("c".to_string(), "3".to_string())
        ]
    );
}

#[test]
fn range_tombstone_suppression() {
    let dir = temp_dir("range_tombstone");
    let mut engine = new_engine(&dir, "test", BIG_MEMTABLE).unwrap();
    engine.set("a", "1").unwrap();
    engine.set("b", "2").unwrap();
    engine.set("c", "3").unwrap();
    engine.delete("b").unwrap();

    let results = engine.range("a", "c").unwrap();
    assert_eq!(
        results,
        vec![
            ("a".to_string(), "1".to_string()),
            ("c".to_string(), "3".to_string())
        ]
    );
}

// --- WAL recovery ---

#[test]
fn wal_recovers_unflushed_writes() {
    let dir = temp_dir("wal_recover");
    {
        let mut engine = new_engine(&dir, "test", BIG_MEMTABLE).unwrap();
        engine.set("k1", "v1").unwrap();
        engine.set("k2", "v2").unwrap();
    }

    let engine = engine_from_dir(&dir, "test", BIG_MEMTABLE).unwrap();
    assert_eq!(
        engine.get("k1").unwrap(),
        Some(("k1".to_string(), "v1".to_string()))
    );
    assert_eq!(
        engine.get("k2").unwrap(),
        Some(("k2".to_string(), "v2".to_string()))
    );
}

#[test]
fn wal_recovers_unflushed_delete() {
    let dir = temp_dir("wal_delete");
    {
        let mut engine = new_engine(&dir, "test", 1).unwrap();
        engine.set("k1", "v1").unwrap();
    }
    {
        let mut engine = engine_from_dir(&dir, "test", BIG_MEMTABLE).unwrap();
        engine.delete("k1").unwrap();
    }

    let engine = engine_from_dir(&dir, "test", BIG_MEMTABLE).unwrap();
    assert_eq!(engine.get("k1").unwrap(), None);
}

// --- Compaction with many keys (exercises cross-level merge) ---

#[test]
fn compact_many_keys_preserves_all() {
    let dir = temp_dir("compact_many");
    let mut engine = new_engine(&dir, "test", 1).unwrap();
    for i in 0..50 {
        engine
            .set(&format!("key{:03}", i), &format!("val{}", i))
            .unwrap();
    }

    engine.compact().unwrap();

    for i in 0..50 {
        let (_, v) = engine
            .get(&format!("key{:03}", i))
            .unwrap()
            .unwrap_or_else(|| panic!("key{:03} missing after compaction", i));
        assert_eq!(v, format!("val{}", i));
    }
}

#[test]
fn compact_overwrites_keep_latest() {
    let dir = temp_dir("compact_overwrite");
    let mut engine = new_engine(&dir, "test", 1).unwrap();
    for round in 0..3 {
        for i in 0..10 {
            engine
                .set(&format!("key{:03}", i), &format!("val_r{}_{}", round, i))
                .unwrap();
        }
    }

    engine.compact().unwrap();

    for i in 0..10 {
        let (_, v) = engine
            .get(&format!("key{:03}", i))
            .unwrap()
            .unwrap_or_else(|| panic!("key{:03} missing", i));
        assert_eq!(v, format!("val_r2_{}", i));
    }
}

#[test]
fn segment_count_after_compact() {
    let dir = temp_dir("seg_count");
    let engine = new_engine(&dir, "test", 1).unwrap();
    engine.set("a", "1").unwrap();
    engine.set("b", "2").unwrap();
    engine.set("c", "3").unwrap();
    drop(engine);

    let engine = engine_from_dir(&dir, "test", 1).unwrap();
    assert!(engine.segment_count() >= 3);

    engine.compact().unwrap();

    // After compact_all, all data cascades to the terminal level as one file
    assert!(
        engine.segment_count() <= 2,
        "expected fewer segments after compaction, got {}",
        engine.segment_count()
    );
}
