use hash_index::bffp::{ResponseStatus, decode_response_frame};
use std::{
    collections::HashMap,
    env, fs,
    io::{Cursor, Read, Write},
    net::{Shutdown, TcpStream},
    path::Path,
    process::Command,
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime},
};

struct ServerProcess {
    child: std::process::Child,
    db_path: String,
}

impl ServerProcess {
    fn start(db_path: &str) -> Self {
        let _ = fs::create_dir_all(db_path);
        let child = Command::new(env!("CARGO_BIN_EXE_hash-index"))
            .arg(db_path)
            .arg("--tcp")
            .arg("0.0.0.0:0")
            .spawn()
            .expect("failed to start server");

        let addr_file = format!("{}/server.addr", db_path);
        let start = Instant::now();
        let timeout = Duration::from_secs(3);
        let addr = loop {
            if Path::new(&addr_file).exists() {
                if let Ok(content) = fs::read_to_string(&addr_file) {
                    break content.trim().to_string();
                }
            }
            if start.elapsed() > timeout {
                panic!("Server did not provide address within timeout");
            }
            thread::sleep(Duration::from_millis(50));
        };
        set_server_addr(addr);
        Self {
            child,
            db_path: db_path.to_string(),
        }
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(format!("{}/server.addr", self.db_path));
    }
}

fn test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

thread_local! {
    static SERVER_ADDR: std::cell::RefCell<String> = std::cell::RefCell::new(String::new());
}

fn set_server_addr(addr: String) {
    SERVER_ADDR.with(|a| *a.borrow_mut() = addr);
}

fn get_server_addr() -> String {
    SERVER_ADDR.with(|a| a.borrow().clone())
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
    let addr = get_server_addr();
    loop {
        if TcpStream::connect(&addr).is_ok() {
            return;
        }
        if start.elapsed() > timeout {
            panic!("Server did not start within timeout");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn build_input_frame(op: u8, key: Option<&str>, value: Option<&str>) -> Vec<u8> {
    let mut payload = Cursor::new(Vec::new());
    payload.write_all(&[op]).unwrap();
    if let Some(k) = key {
        let kb = k.as_bytes();
        payload.write_all(&(kb.len() as u16).to_be_bytes()).unwrap();
        payload.write_all(kb).unwrap();
    }
    if let Some(v) = value {
        let vb = v.as_bytes();
        payload.write_all(&(vb.len() as u32).to_be_bytes()).unwrap();
        payload.write_all(vb).unwrap();
    }
    let payload = payload.into_inner();
    let mut frame = Cursor::new(Vec::new());
    frame
        .write_all(&(payload.len() as u32).to_be_bytes())
        .unwrap();
    frame.write_all(&payload).unwrap();
    frame.into_inner()
}

fn parse_and_build_frame(command: &str) -> Vec<u8> {
    let mut parts = command.splitn(3, ' ');
    match parts.next().unwrap() {
        "READ" => build_input_frame(1, Some(parts.next().unwrap()), None),
        "WRITE" => {
            let key = parts.next().unwrap();
            let value = parts.next().unwrap();
            build_input_frame(2, Some(key), Some(value))
        }
        "DELETE" => build_input_frame(3, Some(parts.next().unwrap()), None),
        "COMPACT" => build_input_frame(4, None, None),
        "STATS" => build_input_frame(5, None, None),
        cmd => panic!("unknown command: {}", cmd),
    }
}

fn decode_response_to_string(buf: &[u8]) -> String {
    let resp = decode_response_frame(buf).expect("decode failed");
    match resp.status {
        ResponseStatus::Ok => resp
            .payload
            .into_iter()
            .next()
            .unwrap_or_else(|| "OK".to_string()),
        ResponseStatus::NotFound => "Not found".to_string(),
        ResponseStatus::Noop => "NOOP".to_string(),
        ResponseStatus::Error => resp.payload.into_iter().next().unwrap_or_default(),
    }
}

fn send_command(command: &str) -> String {
    let addr = get_server_addr();
    let mut stream = TcpStream::connect(&addr).expect("connect failed");
    stream
        .write_all(&parse_and_build_frame(command))
        .expect("write failed");
    stream.shutdown(Shutdown::Write).expect("shutdown failed");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).expect("read failed");
    decode_response_to_string(&buf)
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
