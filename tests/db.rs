use hash_index::db::DB;
use hash_index::settings::FSyncStrategy;
use std::{env, time::SystemTime};

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
    let mut db = DB::new(
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
    let db = DB::new(
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
    let mut db = DB::new(
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
    let mut db = DB::new(
        &temp_db_path("preserve"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    )
    .unwrap();
    db.set("k1", "v1").unwrap();
    db.set("k2", "v2").unwrap();

    let compacted = db.get_compacted().unwrap();

    let (_, v1) = compacted.get("k1").unwrap().unwrap();
    let (_, v2) = compacted.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

#[test]
fn compact_keeps_latest_value() {
    let mut db = DB::new(
        &temp_db_path("latest"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    )
    .unwrap();
    db.set("k1", "v1").unwrap();
    db.set("k1", "v2").unwrap();

    let compacted = db.get_compacted().unwrap();

    let (_, value) = compacted.get("k1").unwrap().unwrap();
    assert_eq!(value, "v2");
}

#[test]
fn compact_drops_deleted_keys() {
    let mut db = DB::new(
        &temp_db_path("deleted"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    )
    .unwrap();
    db.set("k1", "v1").unwrap();
    db.delete("k1").unwrap();

    let compacted = db.get_compacted().unwrap();

    assert_eq!(compacted.get("k1").unwrap(), None);
}

#[test]
fn compact_is_idempotent() {
    let mut db = DB::new(
        &temp_db_path("idempotent"),
        "test",
        DEFAULT_MAX_SEGMENT_BYTES,
        FSyncStrategy::Always,
    )
    .unwrap();
    db.set("k1", "v1").unwrap();
    db.set("k2", "v2").unwrap();

    let compacted = db.get_compacted().unwrap();
    let compacted_again = compacted.get_compacted().unwrap();

    let (_, v1) = compacted_again.get("k1").unwrap().unwrap();
    let (_, v2) = compacted_again.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}

#[test]
fn segment_rolls_when_full() {
    // Use a tiny limit so that a second write triggers a new segment
    let path = temp_db_path("roll");
    let mut db = DB::new(&path, "test", 50, FSyncStrategy::Always).unwrap();
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
        let mut db = DB::new(&path, "test", 50, FSyncStrategy::Always).unwrap();
        db.set("k1", "value_one").unwrap();
        db.set("k2", "value_two").unwrap();
    }

    // Reopen from disk — should rebuild index across all segments
    let db = DB::from_dir(&path, "test", 50, FSyncStrategy::Always)
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
    let mut db = DB::new(&path, "test", 50, FSyncStrategy::Always).unwrap();
    db.set("k1", "value_one").unwrap();
    db.set("k2", "value_two").unwrap();
    db.set("k1", "updated").unwrap();

    let compacted = db.get_compacted().unwrap();
    let (_, v1) = compacted.get("k1").unwrap().unwrap();
    let (_, v2) = compacted.get("k2").unwrap().unwrap();
    assert_eq!(v1, "updated");
    assert_eq!(v2, "value_two");
}

#[test]
fn sync_never_writes_are_readable() {
    let mut db = DB::new(
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
    let mut db = DB::new(
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
    let mut db = DB::new(
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
    let mut db = DB::new(&path, "test", 50, FSyncStrategy::EveryN(2)).unwrap();
    db.set("k1", "value_one").unwrap();
    db.set("k2", "value_two").unwrap();
    db.set("k1", "updated").unwrap();

    let compacted = db.get_compacted().unwrap();
    let (_, v1) = compacted.get("k1").unwrap().unwrap();
    let (_, v2) = compacted.get("k2").unwrap().unwrap();
    assert_eq!(v1, "updated");
    assert_eq!(v2, "value_two");
}
