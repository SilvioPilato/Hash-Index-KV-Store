use rustikv::bffp::{Command, ResponseStatus, decode_response_frame};
use rustikv::engine::StorageEngine;
use rustikv::kvengine::KVEngine;
use rustikv::lsmengine::LsmEngine;
use rustikv::server::{CompactionCfg, dispatch};
use rustikv::settings::FSyncStrategy;
use rustikv::size_tiered::SizeTiered;
use rustikv::stats::Stats;
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
    path.push(format!("kv_ttl_cmd_{}_{}", suffix, nanos));
    fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().to_string()
}

fn kv_engine(suffix: &str) -> Arc<dyn StorageEngine> {
    let dir = temp_dir(suffix);
    Arc::new(KVEngine::new(&dir, "seg", 1024 * 1024, FSyncStrategy::Never).unwrap())
}

fn lsm_engine(suffix: &str) -> Arc<dyn StorageEngine> {
    let dir = temp_dir(suffix);
    let strategy = Box::new(SizeTiered::new(4, 32, 4096, true));
    Arc::new(LsmEngine::new(&dir, "seg", 1_048_576, strategy, 4096, true).unwrap())
}

fn no_compact() -> CompactionCfg {
    CompactionCfg {
        ratio: 0.0,
        max_segment: 0,
    }
}

fn stats() -> Arc<Stats> {
    Arc::new(Stats::new())
}

// ---------------------------------------------------------------------------
// TTL command — KV engine
// ---------------------------------------------------------------------------

#[test]
fn ttl_command_on_existing_key_returns_ok_kv() {
    let engine = kv_engine("ttl_ok_kv");
    let stats = stats();
    dispatch(
        Command::Write("k".to_string(), "v".to_string(), None),
        &engine,
        &stats,
        &no_compact(),
    );
    let resp = dispatch(
        Command::Ttl("k".to_string(), 30),
        &engine,
        &stats,
        &no_compact(),
    );
    let decoded = decode_response_frame(&resp).unwrap();
    assert!(matches!(decoded.status, ResponseStatus::Ok));
}

#[test]
fn ttl_command_on_missing_key_returns_not_found_kv() {
    let engine = kv_engine("ttl_nf_kv");
    let stats = stats();
    let resp = dispatch(
        Command::Ttl("no_such_key".to_string(), 30),
        &engine,
        &stats,
        &no_compact(),
    );
    let decoded = decode_response_frame(&resp).unwrap();
    assert!(matches!(decoded.status, ResponseStatus::NotFound));
}

#[test]
fn ttl_persist_seconds_zero_returns_ok_kv() {
    let engine = kv_engine("ttl_persist_kv");
    let stats = stats();
    dispatch(
        Command::Write("k".to_string(), "v".to_string(), None),
        &engine,
        &stats,
        &no_compact(),
    );
    // seconds == 0 → PERSIST (strip expiry)
    let resp = dispatch(
        Command::Ttl("k".to_string(), 0),
        &engine,
        &stats,
        &no_compact(),
    );
    let decoded = decode_response_frame(&resp).unwrap();
    assert!(matches!(decoded.status, ResponseStatus::Ok));
    // Key is still readable after PERSIST.
    let read_resp = dispatch(
        Command::Read("k".to_string()),
        &engine,
        &stats,
        &no_compact(),
    );
    let read_decoded = decode_response_frame(&read_resp).unwrap();
    assert!(matches!(read_decoded.status, ResponseStatus::Ok));
}

// ---------------------------------------------------------------------------
// TTL command — LSM engine
// ---------------------------------------------------------------------------

#[test]
fn ttl_command_on_existing_key_returns_ok_lsm() {
    let engine = lsm_engine("ttl_ok_lsm");
    let stats = stats();
    dispatch(
        Command::Write("k".to_string(), "v".to_string(), None),
        &engine,
        &stats,
        &no_compact(),
    );
    let resp = dispatch(
        Command::Ttl("k".to_string(), 30),
        &engine,
        &stats,
        &no_compact(),
    );
    let decoded = decode_response_frame(&resp).unwrap();
    assert!(matches!(decoded.status, ResponseStatus::Ok));
    // Key is still readable — TTL is 30 seconds in the future.
    let read_resp = dispatch(
        Command::Read("k".to_string()),
        &engine,
        &stats,
        &no_compact(),
    );
    let read_decoded = decode_response_frame(&read_resp).unwrap();
    assert!(matches!(read_decoded.status, ResponseStatus::Ok));
}

#[test]
fn ttl_command_on_missing_key_returns_not_found_lsm() {
    let engine = lsm_engine("ttl_nf_lsm");
    let stats = stats();
    let resp = dispatch(
        Command::Ttl("no_such_key".to_string(), 30),
        &engine,
        &stats,
        &no_compact(),
    );
    let decoded = decode_response_frame(&resp).unwrap();
    assert!(matches!(decoded.status, ResponseStatus::NotFound));
}

#[test]
fn ttl_persist_seconds_zero_makes_key_permanent_lsm() {
    let engine = lsm_engine("ttl_persist_lsm");
    let stats = stats();
    // Write with a 1-second TTL (still alive), then PERSIST.
    dispatch(
        Command::Write("k".to_string(), "v".to_string(), Some(1)),
        &engine,
        &stats,
        &no_compact(),
    );
    let resp = dispatch(
        Command::Ttl("k".to_string(), 0),
        &engine,
        &stats,
        &no_compact(),
    );
    let decoded = decode_response_frame(&resp).unwrap();
    assert!(matches!(decoded.status, ResponseStatus::Ok));
    // Key must still be visible after PERSIST.
    let read_resp = dispatch(
        Command::Read("k".to_string()),
        &engine,
        &stats,
        &no_compact(),
    );
    let read_decoded = decode_response_frame(&read_resp).unwrap();
    assert!(matches!(read_decoded.status, ResponseStatus::Ok));
    assert_eq!(read_decoded.payload, vec!["v".to_string()]);
}

// ---------------------------------------------------------------------------
// WRITE with TTL (seconds→expiry_ms conversion in dispatch)
// ---------------------------------------------------------------------------

#[test]
fn write_with_future_ttl_is_readable_lsm() {
    let engine = lsm_engine("write_ttl_read_lsm");
    let stats = stats();
    // seconds=60 → expiry_ms = now_ms + 60_000; should be readable immediately.
    let resp = dispatch(
        Command::Write("k".to_string(), "v".to_string(), Some(60)),
        &engine,
        &stats,
        &no_compact(),
    );
    let decoded = decode_response_frame(&resp).unwrap();
    assert!(matches!(decoded.status, ResponseStatus::Ok));

    let read_resp = dispatch(
        Command::Read("k".to_string()),
        &engine,
        &stats,
        &no_compact(),
    );
    let read_decoded = decode_response_frame(&read_resp).unwrap();
    assert!(matches!(read_decoded.status, ResponseStatus::Ok));
    assert_eq!(read_decoded.payload, vec!["v".to_string()]);
}

// Test that an already-expired key written via engine directly is invisible
// on READ through dispatch (LSM only — KV does not filter on read).
#[test]
fn read_expired_key_returns_not_found_lsm() {
    let dir = temp_dir("read_expired_lsm");
    let strategy = Box::new(SizeTiered::new(4, 32, 4096, true));
    let inner = LsmEngine::new(&dir, "seg", 1_048_576, strategy, 4096, true).unwrap();
    let past_ms = now_ms() - 1;
    inner.set_with_ttl("k", "v", Some(past_ms)).unwrap();
    let engine: Arc<dyn StorageEngine> = Arc::new(inner);
    let stats = stats();

    let resp = dispatch(
        Command::Read("k".to_string()),
        &engine,
        &stats,
        &no_compact(),
    );
    let decoded = decode_response_frame(&resp).unwrap();
    assert!(matches!(decoded.status, ResponseStatus::NotFound));
}

// ---------------------------------------------------------------------------
// MSET with TTL
// ---------------------------------------------------------------------------

#[test]
fn mset_with_per_entry_ttl_readable_while_live() {
    let engine = lsm_engine("mset_ttl_lsm");
    let stats = stats();
    let resp = dispatch(
        Command::Mset(vec![
            ("k1".to_string(), "v1".to_string(), Some(60)),
            ("k2".to_string(), "v2".to_string(), None),
        ]),
        &engine,
        &stats,
        &no_compact(),
    );
    let decoded = decode_response_frame(&resp).unwrap();
    assert!(matches!(decoded.status, ResponseStatus::Ok));

    let r1 = decode_response_frame(&dispatch(
        Command::Read("k1".to_string()),
        &engine,
        &stats,
        &no_compact(),
    ))
    .unwrap();
    assert!(matches!(r1.status, ResponseStatus::Ok));
    assert_eq!(r1.payload, vec!["v1"]);

    let r2 = decode_response_frame(&dispatch(
        Command::Read("k2".to_string()),
        &engine,
        &stats,
        &no_compact(),
    ))
    .unwrap();
    assert!(matches!(r2.status, ResponseStatus::Ok));
    assert_eq!(r2.payload, vec!["v2"]);
}
