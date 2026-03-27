use rustikv::bffp::{ResponseStatus, decode_response_frame};
use std::{
    env, fs,
    io::{Cursor, Read, Write},
    net::{Shutdown, TcpStream},
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime},
};

fn temp_db_path(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = env::temp_dir();
    path.push(format!("kv_store_ping_{}_{}", suffix, nanos));
    path.to_string_lossy().to_string()
}

fn start_server(db_path: &str) -> std::process::Child {
    let _ = fs::create_dir_all(db_path);
    Command::new(env!("CARGO_BIN_EXE_rustikv"))
        .arg(db_path)
        .arg("--tcp")
        .arg("0.0.0.0:0")
        .spawn()
        .expect("failed to start server")
}

fn read_server_addr(db_path: &str) -> String {
    let addr_file = format!("{}/server.addr", db_path);
    let start = Instant::now();
    loop {
        if Path::new(&addr_file).exists() {
            if let Ok(content) = fs::read_to_string(&addr_file) {
                return content.trim().to_string();
            }
        }
        if start.elapsed() > Duration::from_secs(3) {
            panic!("Server did not provide address within timeout");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_server(addr: &str) {
    let start = Instant::now();
    loop {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        if start.elapsed() > Duration::from_secs(3) {
            panic!("Server did not start within timeout");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn build_ping_frame() -> Vec<u8> {
    // op code 8 = Ping, no key/value payload
    let op: u8 = 8;
    let payload_len: u32 = 1; // just the op byte
    let mut frame = Cursor::new(Vec::new());
    frame.write_all(&payload_len.to_be_bytes()).unwrap();
    frame.write_all(&[op]).unwrap();
    frame.into_inner()
}

fn send_frame(addr: &str, frame: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(addr).expect("connect failed");
    stream.write_all(frame).expect("write failed");
    stream.shutdown(Shutdown::Write).expect("shutdown failed");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).expect("read failed");
    buf
}

#[test]
fn ping_returns_pong() {
    let db_path = temp_db_path("ping");
    let mut child = start_server(&db_path);
    let addr = read_server_addr(&db_path);
    wait_for_server(&addr);

    let response_bytes = send_frame(&addr, &build_ping_frame());
    let response = decode_response_frame(&response_bytes).expect("decode failed");

    assert!(matches!(response.status, ResponseStatus::Ok));
    assert_eq!(response.payload, vec!["PONG"]);

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(format!("{}/server.addr", db_path));
}
