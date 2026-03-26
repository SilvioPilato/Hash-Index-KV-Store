use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::{env, time::SystemTime};

use rustikv::engine::StorageEngine;
use rustikv::kvengine::KVEngine;
use rustikv::settings::FSyncStrategy;
use rustikv::worker::BackgroundWorker;

fn temp_db_path(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = env::temp_dir();
    path.push(format!("kv_store_worker_{}_{}", suffix, nanos));
    path.to_string_lossy().to_string()
}

#[test]
fn worker_runs_job_periodically() {
    let counter = Arc::new(AtomicU64::new(0));
    let c = Arc::clone(&counter);

    let _worker = BackgroundWorker::spawn(Duration::from_millis(50), move || {
        c.fetch_add(1, Ordering::Relaxed);
    });

    std::thread::sleep(Duration::from_millis(200));
    let count = counter.load(Ordering::Relaxed);
    assert!(count >= 2, "expected at least 2 ticks, got {count}");
}

#[test]
fn worker_stops_on_drop() {
    let counter = Arc::new(AtomicU64::new(0));
    let c = Arc::clone(&counter);

    {
        let _worker = BackgroundWorker::spawn(Duration::from_millis(50), move || {
            c.fetch_add(1, Ordering::Relaxed);
        });
        std::thread::sleep(Duration::from_millis(150));
    } // worker dropped here

    let count_at_drop = counter.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(150));
    let count_after = counter.load(Ordering::Relaxed);

    assert_eq!(
        count_at_drop, count_after,
        "job should not run after worker is dropped"
    );
}

#[test]
fn worker_drop_returns_immediately() {
    let worker = BackgroundWorker::spawn(Duration::from_secs(60), || {});

    let start = std::time::Instant::now();
    drop(worker);
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(1),
        "drop took {elapsed:?}, expected < 1s (unpark should wake the thread immediately)"
    );
}

#[test]
fn periodic_fsync_db_writes_are_readable() {
    let path = temp_db_path("periodic_rw");

    let mut db = KVEngine::new(
        &path,
        "test",
        1_048_576,
        FSyncStrategy::Periodic(Duration::from_millis(100)),
    )
    .unwrap();

    db.set("hello", "world").unwrap();
    let result = db.get("hello").unwrap();
    assert_eq!(result, Some(("hello".to_string(), "world".to_string())));
}

#[test]
fn periodic_fsync_survives_segment_roll() {
    let path = temp_db_path("periodic_roll");

    // Tiny segment size to force a roll
    let mut db = KVEngine::new(
        &path,
        "test",
        50,
        FSyncStrategy::Periodic(Duration::from_millis(100)),
    )
    .unwrap();

    db.set("key1", "value1").unwrap();
    db.set("key2", "value2").unwrap(); // should trigger segment roll

    assert_eq!(
        db.get("key1").unwrap(),
        Some(("key1".to_string(), "value1".to_string()))
    );
    assert_eq!(
        db.get("key2").unwrap(),
        Some(("key2".to_string(), "value2".to_string()))
    );
}
