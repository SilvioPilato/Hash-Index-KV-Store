use rustikv::engine::{RangeScan, StorageEngine};
use rustikv::kvengine::KVEngine;
use rustikv::lsmengine::LsmEngine;
use rustikv::settings::FSyncStrategy;
use rustikv::size_tiered::SizeTiered;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::{env, fs, thread, time::Duration, time::SystemTime};

const DEFAULT_MAX_SEGMENT_BYTES: u64 = 1_048_576 * 50;
const BIG_MEMTABLE: usize = 1_048_576;

fn temp_dir(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tid = thread::current().id();
    let mut path = env::temp_dir();
    path.push(format!("kv_conc_{}_{}_{:?}", suffix, nanos, tid));
    fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().to_string()
}

fn new_kv(dir: &str, max_seg: u64) -> KVEngine {
    KVEngine::new(dir, "test", max_seg, FSyncStrategy::Never).unwrap()
}

fn new_lsm(dir: &str, max_memtable: usize) -> LsmEngine {
    let strategy = Box::new(SizeTiered::new(4, 32, 4096, true));
    LsmEngine::new(dir, "test", max_memtable, strategy, 4096, true).unwrap()
}

// ---------------------------------------------------------------------------
// KVEngine concurrency tests
// ---------------------------------------------------------------------------

#[test]
fn kv_concurrent_readers() {
    let dir = temp_dir("kv_conc_readers");
    let db = Arc::new(new_kv(&dir, DEFAULT_MAX_SEGMENT_BYTES));

    for i in 0..100 {
        db.set(&format!("k{i:03}"), &format!("v{i:03}")).unwrap();
    }

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                for _ in 0..100 {
                    for i in 0..100 {
                        let (_, v) = db.get(&format!("k{i:03}")).unwrap().unwrap();
                        assert_eq!(v, format!("v{i:03}"));
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn kv_concurrent_writers() {
    let dir = temp_dir("kv_conc_writers");
    let db = Arc::new(new_kv(&dir, DEFAULT_MAX_SEGMENT_BYTES));
    let barrier = Arc::new(Barrier::new(8));

    let handles: Vec<_> = (0..8)
        .map(|t| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for i in 0..200 {
                    let key = format!("t{t}_k{i}");
                    let val = format!("t{t}_v{i}");
                    db.set(&key, &val).unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    // Verify all keys are present with correct values
    for t in 0..8 {
        for i in 0..200 {
            let key = format!("t{t}_k{i}");
            let expected = format!("t{t}_v{i}");
            let (_, v) = db.get(&key).unwrap().unwrap();
            assert_eq!(v, expected, "wrong value for key {key}");
        }
    }
}

#[test]
fn kv_concurrent_reads_and_writes() {
    let dir = temp_dir("kv_conc_rw");
    let db = Arc::new(new_kv(&dir, DEFAULT_MAX_SEGMENT_BYTES));

    // Pre-populate so readers always have something to read
    for i in 0..50 {
        db.set(&format!("seed{i:03}"), &format!("val{i:03}"))
            .unwrap();
    }

    let barrier = Arc::new(Barrier::new(6));

    // 4 reader threads
    let mut handles: Vec<_> = (0..4)
        .map(|_| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..200 {
                    for i in 0..50 {
                        let result = db.get(&format!("seed{i:03}")).unwrap();
                        // Value must be the original or an updated version — never corrupted
                        if let Some((_, v)) = result {
                            assert!(
                                v.starts_with("val") || v.starts_with("upd"),
                                "corrupted value: {v}"
                            );
                        }
                    }
                }
            })
        })
        .collect();

    // 2 writer threads overwriting seed keys
    for t in 0..2 {
        let db = Arc::clone(&db);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for round in 0..50 {
                for i in 0..50 {
                    db.set(&format!("seed{i:03}"), &format!("upd{t}_{round}"))
                        .unwrap();
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn kv_concurrent_writes_with_segment_roll() {
    // Tiny segment size forces frequent rolls during concurrent writes
    let dir = temp_dir("kv_conc_roll");
    let db = Arc::new(new_kv(&dir, 200)); // ~200 bytes per segment
    let barrier = Arc::new(Barrier::new(4));

    let handles: Vec<_> = (0..4)
        .map(|t| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for i in 0..100 {
                    let key = format!("t{t}_k{i}");
                    let val = format!("t{t}_v{i}");
                    db.set(&key, &val).unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    // All keys must be readable
    for t in 0..4 {
        for i in 0..100 {
            let key = format!("t{t}_k{i}");
            let expected = format!("t{t}_v{i}");
            let (_, v) = db.get(&key).unwrap().unwrap();
            assert_eq!(v, expected, "wrong value for {key} after concurrent rolls");
        }
    }
}

#[test]
fn kv_concurrent_deletes_and_reads() {
    let dir = temp_dir("kv_conc_del");
    let db = Arc::new(new_kv(&dir, DEFAULT_MAX_SEGMENT_BYTES));

    for i in 0..100 {
        db.set(&format!("k{i:03}"), &format!("v{i:03}")).unwrap();
    }

    let barrier = Arc::new(Barrier::new(4));

    // 2 threads deleting even keys
    let mut handles: Vec<_> = (0..2)
        .map(|t| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                // Each thread deletes half the even keys
                for i in (t..100).step_by(4) {
                    let _ = db.delete(&format!("k{i:03}"));
                }
            })
        })
        .collect();

    // 2 reader threads
    for _ in 0..2 {
        let db = Arc::clone(&db);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for _ in 0..50 {
                for i in 0..100 {
                    // get() must not panic or return corrupted data
                    let result = db.get(&format!("k{i:03}")).unwrap();
                    if let Some((_, v)) = result {
                        assert_eq!(v, format!("v{i:03}"));
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn kv_compact_during_reads_and_writes() {
    let dir = temp_dir("kv_conc_compact");
    let db = Arc::new(new_kv(&dir, DEFAULT_MAX_SEGMENT_BYTES));

    // Pre-populate
    for i in 0..50 {
        db.set(&format!("k{i:03}"), &format!("v{i:03}")).unwrap();
    }

    let done = Arc::new(AtomicBool::new(false));
    let barrier = Arc::new(Barrier::new(4));

    // Reader thread
    let db_r = Arc::clone(&db);
    let done_r = Arc::clone(&done);
    let barrier_r = Arc::clone(&barrier);
    let reader = thread::spawn(move || {
        barrier_r.wait();
        while !done_r.load(Ordering::Relaxed) {
            for i in 0..50 {
                let result = db_r.get(&format!("k{i:03}")).unwrap();
                if let Some((_, v)) = result {
                    assert!(
                        v.starts_with('v') || v.starts_with('w'),
                        "corrupted value during compaction: {v}"
                    );
                }
            }
        }
    });

    // Writer thread
    let db_w = Arc::clone(&db);
    let done_w = Arc::clone(&done);
    let barrier_w = Arc::clone(&barrier);
    let writer = thread::spawn(move || {
        barrier_w.wait();
        let mut round = 0;
        while !done_w.load(Ordering::Relaxed) {
            for i in 0..50 {
                db_w.set(&format!("k{i:03}"), &format!("w{round}_{i}"))
                    .unwrap();
            }
            round += 1;
        }
    });

    // Compaction thread — runs compact multiple times
    let db_c = Arc::clone(&db);
    let barrier_c = Arc::clone(&barrier);
    let compactor = thread::spawn(move || {
        barrier_c.wait();
        for _ in 0..3 {
            db_c.compact().unwrap();
            thread::sleep(Duration::from_millis(10));
        }
    });

    // Let the system run for a bit, gated by compactor
    barrier.wait();
    compactor.join().unwrap();
    done.store(true, Ordering::Relaxed);
    reader.join().unwrap();
    writer.join().unwrap();

    // All keys must still be readable
    for i in 0..50 {
        let result = db.get(&format!("k{i:03}")).unwrap();
        assert!(result.is_some(), "key k{i:03} missing after compaction");
    }
}

// ---------------------------------------------------------------------------
// LsmEngine concurrency tests
// ---------------------------------------------------------------------------

#[test]
fn lsm_concurrent_readers() {
    let dir = temp_dir("lsm_conc_readers");
    let db = Arc::new(new_lsm(&dir, BIG_MEMTABLE));

    for i in 0..100 {
        db.set(&format!("k{i:03}"), &format!("v{i:03}")).unwrap();
    }

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                for _ in 0..100 {
                    for i in 0..100 {
                        let (_, v) = db.get(&format!("k{i:03}")).unwrap().unwrap();
                        assert_eq!(v, format!("v{i:03}"));
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn lsm_concurrent_writers() {
    let dir = temp_dir("lsm_conc_writers");
    let db = Arc::new(new_lsm(&dir, BIG_MEMTABLE));
    let barrier = Arc::new(Barrier::new(8));

    let handles: Vec<_> = (0..8)
        .map(|t| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for i in 0..200 {
                    let key = format!("t{t}_k{i}");
                    let val = format!("t{t}_v{i}");
                    db.set(&key, &val).unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    for t in 0..8 {
        for i in 0..200 {
            let key = format!("t{t}_k{i}");
            let expected = format!("t{t}_v{i}");
            let (_, v) = db.get(&key).unwrap().unwrap();
            assert_eq!(v, expected, "wrong value for key {key}");
        }
    }
}

#[test]
fn lsm_concurrent_reads_and_writes() {
    let dir = temp_dir("lsm_conc_rw");
    let db = Arc::new(new_lsm(&dir, BIG_MEMTABLE));

    for i in 0..50 {
        db.set(&format!("seed{i:03}"), &format!("val{i:03}"))
            .unwrap();
    }

    let barrier = Arc::new(Barrier::new(6));

    let mut handles: Vec<_> = (0..4)
        .map(|_| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..200 {
                    for i in 0..50 {
                        let result = db.get(&format!("seed{i:03}")).unwrap();
                        if let Some((_, v)) = result {
                            assert!(
                                v.starts_with("val") || v.starts_with("upd"),
                                "corrupted value: {v}"
                            );
                        }
                    }
                }
            })
        })
        .collect();

    for t in 0..2 {
        let db = Arc::clone(&db);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for round in 0..50 {
                for i in 0..50 {
                    db.set(&format!("seed{i:03}"), &format!("upd{t}_{round}"))
                        .unwrap();
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn lsm_concurrent_writes_trigger_flush() {
    // Small memtable threshold forces flushes during concurrent writes.
    // This exercises the active/immutable memtable swap under contention.
    let dir = temp_dir("lsm_conc_flush");
    let db = Arc::new(new_lsm(&dir, 256)); // tiny threshold
    let barrier = Arc::new(Barrier::new(4));

    let handles: Vec<_> = (0..4)
        .map(|t| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for i in 0..100 {
                    let key = format!("t{t}_k{i}");
                    let val = format!("t{t}_v{i}");
                    db.set(&key, &val).unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    // Allow background flushes to complete
    thread::sleep(Duration::from_millis(200));

    for t in 0..4 {
        for i in 0..100 {
            let key = format!("t{t}_k{i}");
            let expected = format!("t{t}_v{i}");
            let (_, v) = db.get(&key).unwrap().unwrap();
            assert_eq!(
                v, expected,
                "wrong value for {key} after concurrent flushes"
            );
        }
    }
}

#[test]
fn lsm_concurrent_deletes_and_reads() {
    let dir = temp_dir("lsm_conc_del");
    let db = Arc::new(new_lsm(&dir, BIG_MEMTABLE));

    for i in 0..100 {
        db.set(&format!("k{i:03}"), &format!("v{i:03}")).unwrap();
    }

    let barrier = Arc::new(Barrier::new(4));

    let mut handles: Vec<_> = (0..2)
        .map(|t| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for i in (t..100).step_by(4) {
                    let _ = db.delete(&format!("k{i:03}"));
                }
            })
        })
        .collect();

    for _ in 0..2 {
        let db = Arc::clone(&db);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for _ in 0..50 {
                for i in 0..100 {
                    let result = db.get(&format!("k{i:03}")).unwrap();
                    if let Some((_, v)) = result {
                        assert_eq!(v, format!("v{i:03}"));
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn lsm_compact_during_reads_and_writes() {
    let dir = temp_dir("lsm_conc_compact");
    // Small memtable so data flushes to SSTables for compaction to work on
    let db = Arc::new(new_lsm(&dir, 128));

    // Pre-populate — with threshold=128 these will flush
    for i in 0..50 {
        db.set(&format!("k{i:03}"), &format!("v{i:03}")).unwrap();
    }
    thread::sleep(Duration::from_millis(200)); // let flushes settle

    let done = Arc::new(AtomicBool::new(false));
    let barrier = Arc::new(Barrier::new(4));

    let db_r = Arc::clone(&db);
    let done_r = Arc::clone(&done);
    let barrier_r = Arc::clone(&barrier);
    let reader = thread::spawn(move || {
        barrier_r.wait();
        while !done_r.load(Ordering::Relaxed) {
            for i in 0..50 {
                let result = db_r.get(&format!("k{i:03}")).unwrap();
                if let Some((_, v)) = result {
                    assert!(
                        v.starts_with('v') || v.starts_with('w'),
                        "corrupted value during compaction: {v}"
                    );
                }
            }
        }
    });

    let db_w = Arc::clone(&db);
    let done_w = Arc::clone(&done);
    let barrier_w = Arc::clone(&barrier);
    let writer = thread::spawn(move || {
        barrier_w.wait();
        let mut round = 0;
        while !done_w.load(Ordering::Relaxed) {
            for i in 0..50 {
                db_w.set(&format!("k{i:03}"), &format!("w{round}_{i}"))
                    .unwrap();
            }
            round += 1;
            thread::sleep(Duration::from_millis(5));
        }
    });

    let db_c = Arc::clone(&db);
    let barrier_c = Arc::clone(&barrier);
    let compactor = thread::spawn(move || {
        barrier_c.wait();
        for _ in 0..3 {
            db_c.compact().unwrap();
            thread::sleep(Duration::from_millis(20));
        }
    });

    barrier.wait();
    compactor.join().unwrap();
    done.store(true, Ordering::Relaxed);
    reader.join().unwrap();
    writer.join().unwrap();

    // All keys must still be readable
    for i in 0..50 {
        let result = db.get(&format!("k{i:03}")).unwrap();
        assert!(result.is_some(), "key k{i:03} missing after compaction");
    }
}

#[test]
fn lsm_concurrent_range_scan_during_writes() {
    let dir = temp_dir("lsm_conc_range");
    let db = Arc::new(new_lsm(&dir, BIG_MEMTABLE));

    // Pre-populate keys a..z
    for c in b'a'..=b'z' {
        let key = String::from(c as char);
        db.set(&key, &format!("val_{key}")).unwrap();
    }

    let done = Arc::new(AtomicBool::new(false));
    let barrier = Arc::new(Barrier::new(3));

    // Range scanner
    let db_r = Arc::clone(&db);
    let done_r = Arc::clone(&done);
    let barrier_r = Arc::clone(&barrier);
    let scanner = thread::spawn(move || {
        barrier_r.wait();
        while !done_r.load(Ordering::Relaxed) {
            let results = db_r.range("d", "h").unwrap();
            // Results must be sorted and within bounds
            for (i, (k, _)) in results.iter().enumerate() {
                assert!(k.as_str() >= "d" && k.as_str() <= "h", "out of range: {k}");
                if i > 0 {
                    assert!(
                        results[i - 1].0 < *k,
                        "not sorted: {} >= {k}",
                        results[i - 1].0
                    );
                }
            }
        }
    });

    // Writer overwriting values
    let db_w = Arc::clone(&db);
    let done_w = Arc::clone(&done);
    let barrier_w = Arc::clone(&barrier);
    let writer = thread::spawn(move || {
        barrier_w.wait();
        let mut round = 0;
        while !done_w.load(Ordering::Relaxed) {
            for c in b'a'..=b'z' {
                let key = String::from(c as char);
                db_w.set(&key, &format!("r{round}_{key}")).unwrap();
            }
            round += 1;
        }
    });

    barrier.wait();
    thread::sleep(Duration::from_millis(300));
    done.store(true, Ordering::Relaxed);

    scanner.join().unwrap();
    writer.join().unwrap();
}

#[test]
fn lsm_concurrent_list_keys_during_writes() {
    let dir = temp_dir("lsm_conc_list");
    let db = Arc::new(new_lsm(&dir, BIG_MEMTABLE));

    for i in 0..20 {
        db.set(&format!("k{i:02}"), &format!("v{i}")).unwrap();
    }

    let done = Arc::new(AtomicBool::new(false));
    let barrier = Arc::new(Barrier::new(3));

    // list_keys thread
    let db_l = Arc::clone(&db);
    let done_l = Arc::clone(&done);
    let barrier_l = Arc::clone(&barrier);
    let lister = thread::spawn(move || {
        barrier_l.wait();
        while !done_l.load(Ordering::Relaxed) {
            let keys = db_l.list_keys().unwrap();
            // Must never return duplicates
            let mut sorted = keys.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(
                keys.len(),
                sorted.len(),
                "list_keys returned duplicates: {:?}",
                keys
            );
        }
    });

    // Writer adding new keys
    let db_w = Arc::clone(&db);
    let done_w = Arc::clone(&done);
    let barrier_w = Arc::clone(&barrier);
    let writer = thread::spawn(move || {
        barrier_w.wait();
        for i in 20..120 {
            if done_w.load(Ordering::Relaxed) {
                break;
            }
            db_w.set(&format!("k{i:03}"), &format!("v{i}")).unwrap();
        }
    });

    barrier.wait();
    thread::sleep(Duration::from_millis(300));
    done.store(true, Ordering::Relaxed);

    lister.join().unwrap();
    writer.join().unwrap();
}
