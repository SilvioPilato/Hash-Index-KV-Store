use std::{
    collections::HashMap,
    env,
    io::{Read, Write},
    net::TcpStream,
    process::Command,
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime},
};

struct ServerProcess {
    child: std::process::Child,
}

impl ServerProcess {
    fn start(db_path: &str) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_hash-index"))
            .arg(db_path)
            .spawn()
            .expect("failed to start server");
        Self { child }
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn temp_db_path(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = env::temp_dir();
    path.push(format!("kv_store_async_{}_{}", suffix, nanos));
    path.to_string_lossy().to_string()
}

fn wait_for_server() {
    let start = Instant::now();
    let timeout = Duration::from_secs(3);
    loop {
        if TcpStream::connect("127.0.0.1:6666").is_ok() {
            return;
        }
        if start.elapsed() > timeout {
            panic!("Server did not start within timeout");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn send_command(command: &str) -> String {
    let mut stream = TcpStream::connect("127.0.0.1:6666").expect("connect failed");
    let payload = format!("{}\n\n", command);
    stream.write_all(payload.as_bytes()).expect("write failed");

    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read failed");
    response.trim_end().to_string()
}

fn parse_stats(response: &str) -> HashMap<String, String> {
    response
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn stat_u64(stats: &HashMap<String, String>, key: &str) -> u64 {
    stats
        .get(key)
        .expect("missing stats key")
        .parse::<u64>()
        .expect("invalid stats value")
}

fn wait_for_compacting_state(expected: &str) -> HashMap<String, String> {
    let start = Instant::now();
    let timeout = Duration::from_secs(3);
    loop {
        let stats = parse_stats(&send_command("STATS"));
        if stats.get("compacting").map(|v| v.as_str()) == Some(expected) {
            return stats;
        }
        if start.elapsed() > timeout {
            panic!("compacting flag did not reach {expected} within timeout");
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn stats_show_write_blocked_during_compaction() {
    let _guard = test_lock().lock().unwrap();
    let db_path = temp_db_path("background");

    let _server = ServerProcess::start(&db_path);

    wait_for_server();

    for i in 0..200 {
        let key = format!("k{}", i);
        let value = "x".repeat(10240);
        assert_eq!(send_command(&format!("WRITE {} {}", key, value)), "OK");
    }

    assert_eq!(send_command("COMPACT"), "OK");
    wait_for_compacting_state("true");

    assert_eq!(send_command("WRITE late v"), "OK");
    wait_for_compacting_state("false");

    let stats = parse_stats(&send_command("STATS"));
    assert!(stat_u64(&stats, "write_blocked_attempts") >= 1);
    assert!(stat_u64(&stats, "compaction_count") >= 1);
}

#[test]
fn stats_counters_increment() {
    let _guard = test_lock().lock().unwrap();
    let db_path = temp_db_path("counts");

    let _server = ServerProcess::start(&db_path);

    wait_for_server();

    assert_eq!(send_command("WRITE k1 v1"), "OK");
    assert_eq!(send_command("READ k1"), "v1");
    assert_eq!(send_command("DELETE k1"), "OK");

    let stats = parse_stats(&send_command("STATS"));
    assert!(stat_u64(&stats, "writes") >= 1);
    assert!(stat_u64(&stats, "reads") >= 1);
    assert!(stat_u64(&stats, "deletes") >= 1);
}
