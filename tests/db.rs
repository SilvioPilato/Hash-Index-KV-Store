use hash_index::db::DB;
use std::{env, time::SystemTime};

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
    let mut db = DB::new(&temp_db_path("set_get"), "test");
    db.set("hello", "world").unwrap();
    let (_, value) = db.get("hello").unwrap().unwrap();
    assert_eq!(value, "world");
}

#[test]
fn get_missing_key() {
    let db = DB::new(&temp_db_path("missing"), "test");
    assert_eq!(db.get("nope").unwrap(), None);
}

#[test]
fn set_overwrite() {
    let mut db = DB::new(&temp_db_path("overwrite"), "test");
    db.set("k", "old").unwrap();
    db.set("k", "new").unwrap();
    let (_, value) = db.get("k").unwrap().unwrap();
    assert_eq!(value, "new");
}

#[test]
fn compact_preserves_values() {
    let mut db = DB::new(&temp_db_path("preserve"), "test");
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
    let mut db = DB::new(&temp_db_path("latest"), "test");
    db.set("k1", "v1").unwrap();
    db.set("k1", "v2").unwrap();

    let compacted = db.get_compacted().unwrap();

    let (_, value) = compacted.get("k1").unwrap().unwrap();
    assert_eq!(value, "v2");
}

#[test]
fn compact_drops_deleted_keys() {
    let mut db = DB::new(&temp_db_path("deleted"), "test");
    db.set("k1", "v1").unwrap();
    db.delete("k1").unwrap();

    let compacted = db.get_compacted().unwrap();

    assert_eq!(compacted.get("k1").unwrap(), None);
}

#[test]
fn compact_is_idempotent() {
    let mut db = DB::new(&temp_db_path("idempotent"), "test");
    db.set("k1", "v1").unwrap();
    db.set("k2", "v2").unwrap();

    let compacted = db.get_compacted().unwrap();
    let compacted_again = compacted.get_compacted().unwrap();

    let (_, v1) = compacted_again.get("k1").unwrap().unwrap();
    let (_, v2) = compacted_again.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}
