use rustikv::engine::{StorageEngine, TtlOutcome};
use rustikv::lsmengine::LsmEngine;
use rustikv::size_tiered::SizeTiered;
use rustikv::utils::now_ms;
use std::sync::Arc;
use std::thread;
use std::time::SystemTime;
use std::{env, fs};

fn temp_dir(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = env::temp_dir();
    path.push(format!("kv_lsm_ttl_atomic_{}_{}", suffix, nanos));
    fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().to_string()
}

fn make_engine(suffix: &str) -> LsmEngine {
    let dir = temp_dir(suffix);
    let strategy = Box::new(SizeTiered::new(4, 32, 4096, true));
    LsmEngine::new(&dir, "seg", 1_048_576, strategy, 4096, true).unwrap()
}

// --- ttl() outcome tests ---

#[test]
fn ttl_outcomes() {
    let engine = make_engine("ttl_outcomes");
    assert!(matches!(
        engine.ttl("missing", Some(now_ms() + 10_000)).unwrap(),
        TtlOutcome::NotFound
    ));
    engine.set("k", "v").unwrap();
    assert!(matches!(
        engine.ttl("k", Some(now_ms() + 10_000)).unwrap(),
        TtlOutcome::Set
    ));
    assert!(matches!(
        engine.ttl("k", None).unwrap(),
        TtlOutcome::Persisted
    ));
}

// --- Concurrency: lost-update / resurrection smoke test ---

#[test]
fn ttl_does_not_lose_concurrent_writes() {
    let engine = make_engine("ttl_conc_writes");
    engine.set("k", "v0").unwrap();
    let e = Arc::new(engine);
    let w = {
        let e = e.clone();
        thread::spawn(move || {
            for i in 0..2000 {
                e.set("k", &format!("v{i}")).unwrap();
            }
        })
    };
    let t = {
        let e = e.clone();
        thread::spawn(move || {
            for _ in 0..2000 {
                let _ = e.ttl("k", Some(now_ms() + 60_000));
            }
        })
    };
    w.join().unwrap();
    t.join().unwrap();
    // Invariant: final value is SOME writer value, never resurrected/empty.
    assert!(e.get("k").unwrap().is_some());
}

// --- Deadlock regression: ttl vs compact (validates canonical lock order) ---

#[test]
fn ttl_and_compact_do_not_deadlock() {
    let engine = make_engine("ttl_deadlock");
    for i in 0..200 {
        engine.set(&format!("k{i}"), "v").unwrap();
    }
    let e = Arc::new(engine);
    let c = {
        let e = e.clone();
        thread::spawn(move || {
            for _ in 0..50 {
                let _ = e.compact();
            }
        })
    };
    let t = {
        let e = e.clone();
        thread::spawn(move || {
            for _ in 0..500 {
                let _ = e.ttl("k1", Some(now_ms() + 1_000));
            }
        })
    };
    // If the lock order is wrong, this hangs and the CI timeout kills it.
    c.join().unwrap();
    t.join().unwrap();
}
