use rustikv::engine::{StorageEngine, TtlOutcome};
use rustikv::kvengine::KVEngine;
use rustikv::settings::FSyncStrategy;
use std::sync::Arc;
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
    path.push(format!("kv_kv_ttl_{}_{}", suffix, nanos));
    fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().to_string()
}

fn make_engine(suffix: &str) -> KVEngine {
    let dir = temp_dir(suffix);
    KVEngine::new(&dir, "seg", 1024 * 1024, FSyncStrategy::Never).unwrap()
}

// --- set_with_ttl ---

#[test]
fn set_with_future_ttl_value_is_readable() {
    let engine = make_engine("future_ttl_read");
    let expiry_ms = now_ms() + 60_000; // 60 seconds from now
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

// KVEngine now filters expired keys on read — expired records return None.
#[test]
fn expired_key_invisible_in_kv_engine() {
    let engine = make_engine("expired_invisible");
    let past_ms = now_ms() - 1; // already expired
    engine.set_with_ttl("k", "v", Some(past_ms)).unwrap();
    assert!(engine.get("k").unwrap().is_none());
}

// --- ttl() ---

#[test]
fn ttl_on_existing_key_returns_set() {
    let engine = make_engine("ttl_set");
    engine.set("k", "v").unwrap();
    let expiry_ms = now_ms() + 30_000;
    let outcome = engine.ttl("k", Some(expiry_ms)).unwrap();
    assert!(matches!(outcome, TtlOutcome::Set));
}

#[test]
fn ttl_on_missing_key_returns_not_found() {
    let engine = make_engine("ttl_not_found");
    let expiry_ms = now_ms() + 30_000;
    let outcome = engine.ttl("no_such_key", Some(expiry_ms)).unwrap();
    assert!(matches!(outcome, TtlOutcome::NotFound));
}

#[test]
fn ttl_persist_on_existing_key_returns_persisted() {
    let engine = make_engine("ttl_persist");
    engine.set("k", "v").unwrap();
    // Passing None means "strip the expiry" (PERSIST semantics).
    let outcome = engine.ttl("k", None).unwrap();
    assert!(matches!(outcome, TtlOutcome::Persisted));
}

// --- compact() ---

// KVEngine compaction drops expired records.
#[test]
fn compact_drops_expired_record_in_kv_engine() {
    let dir = temp_dir("compact_expired");
    let engine = KVEngine::new(&dir, "seg", 50, FSyncStrategy::Never).unwrap();
    let past_ms = now_ms() - 1;
    engine.set_with_ttl("k", "v", Some(past_ms)).unwrap();
    engine.compact().unwrap();
    assert!(engine.get("k").unwrap().is_none());
}

#[test]
fn compact_preserves_live_ttl_key() {
    let dir = temp_dir("compact_live_ttl");
    let engine = KVEngine::new(&dir, "seg", 50, FSyncStrategy::Never).unwrap();
    let future_ms = now_ms() + 60_000;
    engine.set_with_ttl("k", "v", Some(future_ms)).unwrap();
    engine.compact().unwrap();
    let (_, val) = engine.get("k").unwrap().unwrap();
    assert_eq!(val, "v");
}

// --- concurrency ---

#[test]
fn concurrent_write_and_ttl_no_lost_update() {
    let dir = temp_dir("kv_conc_ttl");
    let engine = Arc::new(KVEngine::new(&dir, "seg", 1_000_000, FSyncStrategy::Never).unwrap());
    engine.set("k", "original").unwrap();

    let e1 = Arc::clone(&engine);
    let e2 = Arc::clone(&engine);

    let t1 = std::thread::spawn(move || {
        let expiry = now_ms() + 60_000;
        e1.ttl("k", Some(expiry)).unwrap();
    });
    let t2 = std::thread::spawn(move || {
        e2.set("k", "updated").unwrap();
    });

    t1.join().unwrap();
    t2.join().unwrap();

    // Key must exist (one of the two writes wins).
    assert!(engine.get("k").unwrap().is_some());
}

#[test]
fn deadlock_regression_compact_vs_ttl() {
    let dir = temp_dir("kv_deadlock_ttl");
    let engine = Arc::new(KVEngine::new(&dir, "seg", 100, FSyncStrategy::Never).unwrap());
    for i in 0..20 {
        engine.set(&format!("k{i}"), "value").unwrap();
    }

    let e1 = Arc::clone(&engine);
    let e2 = Arc::clone(&engine);

    let t1 = std::thread::spawn(move || {
        e1.compact().unwrap();
    });
    let t2 = std::thread::spawn(move || {
        let expiry = now_ms() + 60_000;
        e2.ttl("k0", Some(expiry)).unwrap();
    });

    t1.join().unwrap();
    t2.join().unwrap();
}
