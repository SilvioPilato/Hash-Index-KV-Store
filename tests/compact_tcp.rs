use std::{
    env, fs,
    io::{Read, Write},
    net::TcpStream,
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

        // Wait for server.addr file to be created
        let addr_file = format!("{}/server.addr", db_path);
        eprintln!("Waiting for address file at: {}", addr_file);
        let start = Instant::now();
        let timeout = Duration::from_secs(3);
        let addr = loop {
            if Path::new(&addr_file).exists() {
                if let Ok(content) = fs::read_to_string(&addr_file) {
                    eprintln!("Found address: {}", content.trim());
                    break content.trim().to_string();
                }
            }
            if start.elapsed() > timeout {
                eprintln!("Timeout: address file not found");
                eprintln!("Directory contents:");
                if let Ok(entries) = fs::read_dir(db_path) {
                    for entry in entries {
                        if let Ok(entry) = entry {
                            eprintln!("  {}", entry.path().display());
                        }
                    }
                }
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

fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock = LOCK.get_or_init(|| Mutex::new(()));
    lock.lock().unwrap_or_else(|e| e.into_inner())
}

thread_local! {
    static SERVER_ADDR: std::cell::RefCell<String> = std::cell::RefCell::new(String::new());
}

fn set_server_addr(addr: String) {
    SERVER_ADDR.with(|a| {
        *a.borrow_mut() = addr;
    });
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
    path.push(format!("kv_store_tcp_{}_{}", suffix, nanos));
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

fn send_command(command: &str) -> String {
    let addr = get_server_addr();
    let mut stream = TcpStream::connect(&addr).expect("connect failed");
    let payload = format!("{}\n\n", command);
    stream.write_all(payload.as_bytes()).expect("write failed");

    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read failed");
    response.trim_end().to_string()
}

fn wait_for_compaction() {
    let start = Instant::now();
    let timeout = Duration::from_secs(5);
    loop {
        let stats = send_command("STATS");
        if !stats.contains("compacting=true") {
            return;
        }
        if start.elapsed() > timeout {
            panic!("Compaction did not finish within timeout");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn compact_command_over_tcp() {
    let _guard = test_lock();
    let db_path = temp_db_path("compact");

    let _server = ServerProcess::start(&db_path);

    wait_for_server();

    assert_eq!(send_command("COMPACT"), "OK");
    assert_eq!(send_command("WRITE k1 v1"), "OK");
    assert_eq!(send_command("COMPACT"), "OK");
    wait_for_compaction();
    assert_eq!(send_command("READ k1"), "v1");
}

#[test]
fn write_preserves_multiple_spaces_in_value() {
    let _guard = test_lock();
    let db_path = temp_db_path("spaces");

    let _server = ServerProcess::start(&db_path);

    wait_for_server();

    assert_eq!(send_command("WRITE key hello  world"), "OK");
    assert_eq!(send_command("READ key"), "hello  world");
}

#[test]
fn write_preserves_leading_and_trailing_spaces_in_value() {
    let _guard = test_lock();
    let db_path = temp_db_path("leading_trailing");

    let _server = ServerProcess::start(&db_path);

    wait_for_server();

    assert_eq!(send_command("WRITE key  leading"), "OK");
    assert_eq!(send_command("READ key"), " leading");
}

#[test]
fn write_preserves_tab_in_value() {
    let _guard = test_lock();
    let db_path = temp_db_path("tabs");

    let _server = ServerProcess::start(&db_path);

    wait_for_server();

    assert_eq!(send_command("WRITE key hello\tworld"), "OK");
    assert_eq!(send_command("READ key"), "hello\tworld");
}
