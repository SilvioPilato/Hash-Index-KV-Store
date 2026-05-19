use rustikv::engine::{StorageEngine, TtlOutcome};
use rustikv::lsmengine::LsmEngine;
use rustikv::size_tiered::SizeTiered;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn temp_dir(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = env::temp_dir();
    path.push(format!("kv_lsm_ttl_{}_{}", suffix, nanos));
    fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().to_string()
}

fn make_engine(suffix: &str) -> LsmEngine {
    let dir = temp_dir(suffix);
    let strategy = Box::new(SizeTiered::new(4, 32, 4096, true));
    LsmEngine::new(&dir, "seg", 1_048_576, strategy, 4096, true).unwrap()
}

// --- set_with_ttl (read path) ---

#[test]
fn set_with_future_ttl_value_is_readable() {
    let engine = make_engine("future_ttl_read");
    let expiry_ms = now_ms() + 60_000;
    engine.set_with_ttl("k", "v", Some(expiry_ms)).unwrap();
    let (_, val) = engine.get("k").unwrap().unwrap();
    assert_eq!(val, "v");
}

#[test]
fn set_with_no_ttl_value_is_readable() {
    let engine = make_engine("no_ttl_read");
    engine.set_with_ttl("k", "v", None).unwrap();
    let (_, val) = engine.get("k").unwrap().unwrap();
    assert_eq!(val, "v");
}

#[test]
fn expired_key_is_invisible() {
    let engine = make_engine("expired_invisible");
    let past_ms = now_ms() - 1;
    engine.set_with_ttl("k", "v", Some(past_ms)).unwrap();
    assert_eq!(engine.get("k").unwrap(), None);
}

#[test]
fn non_expired_and_expired_keys_coexist() {
    let engine = make_engine("mixed_ttl");
    let past_ms = now_ms() - 1;
    let future_ms = now_ms() + 60_000;
    engine
        .set_with_ttl("expired", "gone", Some(past_ms))
        .unwrap();
    engine
        .set_with_ttl("live", "here", Some(future_ms))
        .unwrap();
    engine.set("permanent", "stays").unwrap();

    assert_eq!(engine.get("expired").unwrap(), None);
    let (_, v) = engine.get("live").unwrap().unwrap();
    assert_eq!(v, "here");
    let (_, v) = engine.get("permanent").unwrap().unwrap();
    assert_eq!(v, "stays");
}

// --- ttl() ---

#[test]
fn ttl_on_existing_key_returns_set() {
    let engine = make_engine("ttl_set");
    engine.set("k", "v").unwrap();
    let expiry_ms = now_ms() + 30_000;
    let outcome = engine.ttl("k", Some(expiry_ms)).unwrap();
    assert!(matches!(outcome, TtlOutcome::Set));
    // Key is still readable with future expiry.
    assert!(engine.get("k").unwrap().is_some());
}

#[test]
fn ttl_sets_expiry_key_becomes_invisible_after_past_ms() {
    let engine = make_engine("ttl_expires");
    engine.set("k", "v").unwrap();
    // Apply a past expiry — key should now be invisible.
    let past_ms = now_ms() - 1;
    engine.ttl("k", Some(past_ms)).unwrap();
    assert_eq!(engine.get("k").unwrap(), None);
}

#[test]
fn ttl_on_missing_key_returns_not_found() {
    let engine = make_engine("ttl_not_found");
    let expiry_ms = now_ms() + 30_000;
    let outcome = engine.ttl("no_such_key", Some(expiry_ms)).unwrap();
    assert!(matches!(outcome, TtlOutcome::NotFound));
}

#[test]
fn ttl_persist_makes_expiring_key_permanent() {
    let engine = make_engine("ttl_persist");
    let future_ms = now_ms() + 100; // short but still future
    engine.set_with_ttl("k", "v", Some(future_ms)).unwrap();
    // Persist: strip the expiry.
    let outcome = engine.ttl("k", None).unwrap();
    assert!(matches!(outcome, TtlOutcome::Persisted));
    // Even though the original TTL is very short, the key should now be
    // permanent (no expiry).
    let (_, val) = engine.get("k").unwrap().unwrap();
    assert_eq!(val, "v");
}

// --- compact() ---

#[test]
fn compact_drops_expired_records() {
    let dir = temp_dir("compact_expired");
    let strategy = Box::new(SizeTiered::new(4, 32, 4096, true));
    // Tiny memtable so a flush happens before compaction.
    let engine = LsmEngine::new(&dir, "seg", 1, strategy, 4096, true).unwrap();
    let past_ms = now_ms() - 1;
    engine
        .set_with_ttl("expired", "gone", Some(past_ms))
        .unwrap();
    engine.set("live", "here").unwrap();
    engine.compact().unwrap();
    assert_eq!(engine.get("expired").unwrap(), None);
    let (_, v) = engine.get("live").unwrap().unwrap();
    assert_eq!(v, "here");
}

#[test]
fn compact_retains_non_expired_ttl_key() {
    let dir = temp_dir("compact_live_ttl");
    let strategy = Box::new(SizeTiered::new(4, 32, 4096, true));
    let engine = LsmEngine::new(&dir, "seg", 1, strategy, 4096, true).unwrap();
    let future_ms = now_ms() + 60_000;
    engine.set_with_ttl("k", "v", Some(future_ms)).unwrap();
    engine.compact().unwrap();
    let (_, val) = engine.get("k").unwrap().unwrap();
    assert_eq!(val, "v");
}

#[test]
fn compact_preserves_permanent_keys() {
    let dir = temp_dir("compact_perm");
    let strategy = Box::new(SizeTiered::new(4, 32, 4096, true));
    let engine = LsmEngine::new(&dir, "seg", 1, strategy, 4096, true).unwrap();
    engine.set("k1", "v1").unwrap();
    engine.set("k2", "v2").unwrap();
    engine.compact().unwrap();
    let (_, v1) = engine.get("k1").unwrap().unwrap();
    let (_, v2) = engine.get("k2").unwrap().unwrap();
    assert_eq!(v1, "v1");
    assert_eq!(v2, "v2");
}
