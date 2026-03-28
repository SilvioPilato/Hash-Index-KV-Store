use rustikv::bffp::{Command, ResponseStatus, decode_response_frame, encode_command};
use std::{
    env, fs,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    path::Path,
    process, thread,
    time::{Duration, Instant, SystemTime},
};

fn temp_db_path(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = env::temp_dir();
    path.push(format!("kv_store_mget_mset_{}_{}", suffix, nanos));
    path.to_string_lossy().to_string()
}

fn start_server(db_path: &str) -> process::Child {
    let _ = fs::create_dir_all(db_path);
    process::Command::new(env!("CARGO_BIN_EXE_rustikv"))
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

fn send_frame(addr: &str, frame: Vec<u8>) -> Vec<u8> {
    let mut stream = TcpStream::connect(addr).expect("connect failed");
    stream.write_all(&frame).expect("write failed");
    stream.shutdown(Shutdown::Write).expect("shutdown failed");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).expect("read failed");
    buf
}

fn write_key(addr: &str, key: &str, value: &str) {
    let frame = encode_command(Command::Write(key.to_string(), value.to_string()));
    let buf = send_frame(addr, frame);
    let resp = decode_response_frame(&buf).expect("decode failed");
    assert!(matches!(resp.status, ResponseStatus::Ok));
}

#[test]
fn mget_returns_values_for_all_found_keys() {
    let db_path = temp_db_path("mget_all_found");
    let mut child = start_server(&db_path);
    let addr = read_server_addr(&db_path);
    wait_for_server(&addr);

    write_key(&addr, "k1", "v1");
    write_key(&addr, "k2", "v2");
    write_key(&addr, "k3", "v3");

    let frame = encode_command(Command::Mget(vec![
        "k1".to_string(),
        "k2".to_string(),
        "k3".to_string(),
    ]));
    let buf = send_frame(&addr, frame);
    let resp = decode_response_frame(&buf).expect("decode failed");

    assert!(matches!(resp.status, ResponseStatus::Ok));
    assert_eq!(resp.payload, vec!["k1", "v1", "k2", "v2", "k3", "v3"]);

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(format!("{}/server.addr", db_path));
}

#[test]
fn mget_returns_null_sentinel_for_missing_keys() {
    let db_path = temp_db_path("mget_missing");
    let mut child = start_server(&db_path);
    let addr = read_server_addr(&db_path);
    wait_for_server(&addr);

    write_key(&addr, "k1", "v1");

    let frame = encode_command(Command::Mget(vec!["k1".to_string(), "k2".to_string()]));
    let buf = send_frame(&addr, frame);
    let resp = decode_response_frame(&buf).expect("decode failed");

    assert!(matches!(resp.status, ResponseStatus::Ok));
    assert_eq!(resp.payload, vec!["k1", "v1", "k2", "\0"]);

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(format!("{}/server.addr", db_path));
}

#[test]
fn mset_writes_multiple_keys_readable_individually() {
    let db_path = temp_db_path("mset_write");
    let mut child = start_server(&db_path);
    let addr = read_server_addr(&db_path);
    wait_for_server(&addr);

    let frame = encode_command(Command::Mset(vec![
        ("k1".to_string(), "v1".to_string()),
        ("k2".to_string(), "v2".to_string()),
        ("k3".to_string(), "v3".to_string()),
    ]));
    let buf = send_frame(&addr, frame);
    let resp = decode_response_frame(&buf).expect("decode failed");
    assert!(matches!(resp.status, ResponseStatus::Ok));

    for (key, expected) in [("k1", "v1"), ("k2", "v2"), ("k3", "v3")] {
        let frame = encode_command(Command::Read(key.to_string()));
        let buf = send_frame(&addr, frame);
        let resp = decode_response_frame(&buf).expect("decode failed");
        assert!(matches!(resp.status, ResponseStatus::Ok));
        assert_eq!(resp.payload, vec![expected]);
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(format!("{}/server.addr", db_path));
}

#[test]
fn mset_overwrites_existing_keys() {
    let db_path = temp_db_path("mset_overwrite");
    let mut child = start_server(&db_path);
    let addr = read_server_addr(&db_path);
    wait_for_server(&addr);

    write_key(&addr, "k1", "old");

    let frame = encode_command(Command::Mset(vec![("k1".to_string(), "new".to_string())]));
    let buf = send_frame(&addr, frame);
    let resp = decode_response_frame(&buf).expect("decode failed");
    assert!(matches!(resp.status, ResponseStatus::Ok));

    let frame = encode_command(Command::Read("k1".to_string()));
    let buf = send_frame(&addr, frame);
    let resp = decode_response_frame(&buf).expect("decode failed");
    assert!(matches!(resp.status, ResponseStatus::Ok));
    assert_eq!(resp.payload, vec!["new"]);

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(format!("{}/server.addr", db_path));
}
