use rustikv::engine::StorageEngine;
use rustikv::kvengine::KVEngine;
use rustikv::settings::FSyncStrategy;
use std::{env, sync::Arc, thread, time::SystemTime};

const DEFAULT_MAX_SEGMENT_BYTES: u64 = 1_048_576 * 50;

fn temp_db_path(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = env::temp_dir();
    path.push(format!("kv_store_{}_{}", suffix, nanos));
    path.to_string_lossy().to_string()
}

#[test]
fn set_and_get() {
    let mut db = KVEngine::new(
        &temp_db_path("set_get"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    )
    .unwrap();
    db.set("hello", "world").unwrap();
    let (_, value) = db.get("hello").unwrap().unwrap();
    assert_eq!(value, "world");
}

#[test]
fn get_missing_key() {
    let db = KVEngine::new(
        &temp_db_path("missing"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    )
    .unwrap();
    assert_eq!(db.get("nope").unwrap(), None);
}

#[test]
fn set_overwrite() {
    let mut db = KVEngine::new(
        &temp_db_path("overwrite"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    )
    .unwrap();
    db.set("k", "old").unwrap();
    db.set("k", "new").unwrap();
    let (_, value) = db.get("k").unwrap().unwrap();
    assert_eq!(value, "new");
}

#[test]
fn compact_preserves_values() {
    let mut db = KVEngine::new(
        &temp_db_path("preserve"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    )
    .unwrap();
    db.set("k1", "v1").unwrap();
    db.set("k2", "v2").unwrap();

    db.compact().unwrap();

    let (_, v1) = db.get("k1").unwrap().unwrap();
    let (_, v2) = db.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

#[test]
fn compact_keeps_latest_value() {
    let mut db = KVEngine::new(
        &temp_db_path("latest"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    )
    .unwrap();
    db.set("k1", "v1").unwrap();
    db.set("k1", "v2").unwrap();

    db.compact().unwrap();

    let (_, value) = db.get("k1").unwrap().unwrap();
    assert_eq!(value, "v2");
}

#[test]
fn compact_drops_deleted_keys() {
    let mut db = KVEngine::new(
        &temp_db_path("deleted"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    )
    .unwrap();
    db.set("k1", "v1").unwrap();
    db.delete("k1").unwrap();

    db.compact().unwrap();

    assert_eq!(db.get("k1").unwrap(), None);
}

#[test]
fn compact_is_idempotent() {
    let mut db = KVEngine::new(
        &temp_db_path("idempotent"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    )
    .unwrap();
    db.set("k1", "v1").unwrap();
    db.set("k2", "v2").unwrap();

    db.compact().unwrap();
    db.compact().unwrap();

    let (_, v1) = db.get("k1").unwrap().unwrap();
    let (_, v2) = db.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

#[test]
fn segment_rolls_when_full() {
    // Use a tiny limit so that a second write triggers a new segment
    let path = temp_db_path("roll");
    let mut db = KVEngine::new(&path, "test", 50, FSyncStrategy::Always).unwrap();
    db.set("k1", "value_one").unwrap();
    db.set("k2", "value_two").unwrap();

    // Both keys should be readable even though they're in different segments
    let (_, v1) = db.get("k1").unwrap().unwrap();
    let (_, v2) = db.get("k2").unwrap().unwrap();
    assert_eq!(v1, "value_one");
    assert_eq!(v2, "value_two");
}

#[test]
fn from_dir_loads_all_segments() {
    let path = temp_db_path("from_dir_multi");
    {
        let mut db = KVEngine::new(&path, "test", 50, FSyncStrategy::Always).unwrap();
        db.set("k1", "value_one").unwrap();
        db.set("k2", "value_two").unwrap();
    }

    // Reopen from disk — should rebuild index across all segments
    let db = KVEngine::from_dir(&path, "test", 50, FSyncStrategy::Always)
        .unwrap()
        .unwrap();
    let (_, v1) = db.get("k1").unwrap().unwrap();
    let (_, v2) = db.get("k2").unwrap().unwrap();
    assert_eq!(v1, "value_one");
    assert_eq!(v2, "value_two");
}

#[test]
fn compact_merges_segments() {
    let path = temp_db_path("compact_merge");
    let mut db = KVEngine::new(&path, "test", 50, FSyncStrategy::Always).unwrap();
    db.set("k1", "value_one").unwrap();
    db.set("k2", "value_two").unwrap();
    db.set("k1", "updated").unwrap();

    db.compact().unwrap();
    let (_, v1) = db.get("k1").unwrap().unwrap();
    let (_, v2) = db.get("k2").unwrap().unwrap();
    assert_eq!(v1, "updated");
    assert_eq!(v2, "value_two");
}

#[test]
fn sync_never_writes_are_readable() {
    let mut db = KVEngine::new(
        &temp_db_path("sync_never"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Never,
    )
    .unwrap();
    db.set("k1", "v1").unwrap();
    db.set("k2", "v2").unwrap();
    let (_, v1) = db.get("k1").unwrap().unwrap();
    let (_, v2) = db.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

#[test]
fn sync_every_n_writes_are_readable() {
    let mut db = KVEngine::new(
        &temp_db_path("sync_every_n"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::EveryN(3),
    )
    .unwrap();
    for i in 0..10 {
        db.set(&format!("k{i}"), &format!("v{i}")).unwrap();
    }
    for i in 0..10 {
        let (_, val) = db.get(&format!("k{i}")).unwrap().unwrap();
        assert_eq!(val, format!("v{i}"));
    }
}

#[test]
fn sync_never_delete_works() {
    let mut db = KVEngine::new(
        &temp_db_path("sync_never_del"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Never,
    )
    .unwrap();
    db.set("k1", "v1").unwrap();
    db.delete("k1").unwrap();
    assert_eq!(db.get("k1").unwrap(), None);
}

#[test]
fn sync_every_n_compaction_preserves_data() {
    let path = temp_db_path("sync_every_n_compact");
    let mut db = KVEngine::new(&path, "test", 50, FSyncStrategy::EveryN(2)).unwrap();
    db.set("k1", "value_one").unwrap();
    db.set("k2", "value_two").unwrap();
    db.set("k1", "updated").unwrap();

    db.compact().unwrap();
    let (_, v1) = db.get("k1").unwrap().unwrap();
    let (_, v2) = db.get("k2").unwrap().unwrap();
    assert_eq!(v1, "updated");
    assert_eq!(v2, "value_two");
}

#[test]
fn concurrent_reads_return_correct_values() {
    let mut db = KVEngine::new(
        &temp_db_path("concurrent_reads"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Never,
    )
    .unwrap();

    // Write keys with varying value sizes so records land at different offsets.
    // Offset spread is what causes interleaved seek+read to corrupt results on
    // Linux where try_clone() / dup() shares the file offset across clones.
    let num_keys = 100usize;
    for i in 0..num_keys {
        let key = format!("key_{i:03}");
        let value = format!("value_{}", "x".repeat(i % 64 + 1));
        db.set(&key, &value).unwrap();
    }

    let expected: Arc<Vec<(String, String)>> = Arc::new(
        (0..num_keys)
            .map(|i| {
                let key = format!("key_{i:03}");
                let value = format!("value_{}", "x".repeat(i % 64 + 1));
                (key, value)
            })
            .collect(),
    );

    let db = Arc::new(db);

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let db = Arc::clone(&db);
            let expected = Arc::clone(&expected);
            thread::spawn(move || {
                for _ in 0..200 {
                    for (key, expected_value) in expected.iter() {
                        let (_, actual) = db.get(key).unwrap().unwrap();
                        assert_eq!(
                            actual, *expected_value,
                            "concurrent read returned wrong value for key '{key}'"
                        );
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn list_keys_returns_all_live_keys() {
    let mut db = KVEngine::new(
        &temp_db_path("list_keys"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Never,
    )
    .unwrap();
    db.set("a", "1").unwrap();
    db.set("b", "2").unwrap();
    db.set("c", "3").unwrap();

    let mut keys = db.list_keys().unwrap();
    keys.sort();
    assert_eq!(keys, vec!["a", "b", "c"]);
}

#[test]
fn list_keys_excludes_deleted_keys() {
    let mut db = KVEngine::new(
        &temp_db_path("list_keys_delete"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Never,
    )
    .unwrap();
    db.set("a", "1").unwrap();
    db.set("b", "2").unwrap();
    db.set("c", "3").unwrap();
    db.delete("b").unwrap();

    let mut keys = db.list_keys().unwrap();
    keys.sort();
    assert_eq!(keys, vec!["a", "c"]);
}

#[test]
fn list_keys_deduplicates_overwritten_keys() {
    let mut db = KVEngine::new(
        &temp_db_path("list_keys_overwrite"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Never,
    )
    .unwrap();
    db.set("k", "old").unwrap();
    db.set("k", "new").unwrap();

    let keys = db.list_keys().unwrap();
    assert_eq!(keys, vec!["k"]);
}

#[test]
fn list_keys_empty_db() {
    let db = KVEngine::new(
        &temp_db_path("list_keys_empty"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Never,
    )
    .unwrap();
    assert_eq!(db.list_keys().unwrap(), Vec::<String>::new());
}

#[test]
fn exists_returns_true_after_set() {
    let mut db = KVEngine::new(
        &temp_db_path("exists_true"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Never,
    )
    .unwrap();
    db.set("k", "v").unwrap();
    assert!(db.exists("k"));
}

#[test]
fn exists_returns_false_for_missing_key() {
    let db = KVEngine::new(
        &temp_db_path("exists_missing"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Never,
    )
    .unwrap();
    assert!(!db.exists("nope"));
}

#[test]
fn exists_returns_false_after_delete() {
    let mut db = KVEngine::new(
        &temp_db_path("exists_delete"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Never,
    )
    .unwrap();
    db.set("k", "v").unwrap();
    db.delete("k").unwrap();
    assert!(!db.exists("k"));
}

// --- Concurrency tests ---

#[test]
fn concurrent_writes_no_lost_keys() {
    let engine = Arc::new(
        KVEngine::new(
            &temp_db_path("conc_writes"),
            "test",
            DEFAULT_MAX_SEGMENT_BYTES,
            FSyncStrategy::Never,
        )
        .unwrap(),
    );
    let num_threads = 8;
    let keys_per_thread = 200;

    let handles: Vec<_> = (0..num_threads)
        .map(|t| {
            let db = Arc::clone(&engine);
            thread::spawn(move || {
                for i in 0..keys_per_thread {
                    let key = format!("t{t}_k{i}");
                    let value = format!("t{t}_v{i}");
                    db.set(&key, &value).unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    for t in 0..num_threads {
        for i in 0..keys_per_thread {
            let key = format!("t{t}_k{i}");
            let expected = format!("t{t}_v{i}");
            let (_, actual) = engine.get(&key).unwrap().unwrap();
            assert_eq!(actual, expected, "missing or wrong value for {key}");
        }
    }
}

#[test]
fn concurrent_reads_and_writes() {
    let engine = Arc::new(
        KVEngine::new(
            &temp_db_path("conc_rw"),
            "test",
            DEFAULT_MAX_SEGMENT_BYTES,
            FSyncStrategy::Never,
        )
        .unwrap(),
    );

    // Pre-populate so readers always find something
    for i in 0..50 {
        engine.set(&format!("k{i}"), &format!("v{i}")).unwrap();
    }

    let writers: Vec<_> = (0..4)
        .map(|t| {
            let db = Arc::clone(&engine);
            thread::spawn(move || {
                for i in 0..200 {
                    let key = format!("w{t}_{i}");
                    db.set(&key, &format!("val_{i}")).unwrap();
                }
            })
        })
        .collect();

    let readers: Vec<_> = (0..4)
        .map(|_| {
            let db = Arc::clone(&engine);
            thread::spawn(move || {
                for round in 0..200 {
                    let key = format!("k{}", round % 50);
                    let result = db.get(&key).unwrap();
                    assert!(result.is_some(), "pre-populated key {key} must exist");
                }
            })
        })
        .collect();

    for h in writers.into_iter().chain(readers) {
        h.join().unwrap();
    }
}

#[test]
fn concurrent_delete_and_read() {
    let engine = Arc::new(
        KVEngine::new(
            &temp_db_path("conc_delete"),
            "test",
            DEFAULT_MAX_SEGMENT_BYTES,
            FSyncStrategy::Never,
        )
        .unwrap(),
    );

    for i in 0..100 {
        engine.set(&format!("k{i}"), &format!("v{i}")).unwrap();
    }

    let deleter = {
        let db = Arc::clone(&engine);
        thread::spawn(move || {
            for i in 0..100 {
                db.delete(&format!("k{i}")).unwrap();
            }
        })
    };

    let readers: Vec<_> = (0..4)
        .map(|_| {
            let db = Arc::clone(&engine);
            thread::spawn(move || {
                for _ in 0..100 {
                    for i in 0..100 {
                        let key = format!("k{i}");
                        match db.get(&key).unwrap() {
                            Some((_, v)) => assert_eq!(v, format!("v{i}")),
                            None => {} // deleted, fine
                        }
                    }
                }
            })
        })
        .collect();

    deleter.join().unwrap();
    for h in readers {
        h.join().unwrap();
    }
}

#[test]
fn concurrent_overwrite_last_writer_wins() {
    let engine = Arc::new(
        KVEngine::new(
            &temp_db_path("conc_overwrite"),
            "test",
            DEFAULT_MAX_SEGMENT_BYTES,
            FSyncStrategy::Never,
        )
        .unwrap(),
    );

    let handles: Vec<_> = (0..8)
        .map(|t| {
            let db = Arc::clone(&engine);
            thread::spawn(move || {
                for i in 0..500 {
                    db.set("contested", &format!("t{t}_i{i}")).unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let (_, val) = engine.get("contested").unwrap().unwrap();
    assert!(
        val.starts_with('t'),
        "value should come from a writer thread: {val}"
    );
}
