use std::{
    env,
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use rustikv::bffp::{Command, ResponseStatus, decode_response_frame, encode_command};

fn send_command(stream: &mut TcpStream, cmd: Command) -> io::Result<()> {
    let frame = encode_command(cmd);
    stream.write_all(&frame)?;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let frame_len = u32::from_be_bytes(len_buf) as usize;

    let mut payload = vec![0u8; frame_len];
    stream.read_exact(&mut payload)?;

    let mut full = Vec::with_capacity(4 + frame_len);
    full.extend_from_slice(&len_buf);
    full.extend_from_slice(&payload);

    let res = decode_response_frame(&full)?;
    match res.status {
        ResponseStatus::Ok | ResponseStatus::Noop | ResponseStatus::NotFound => Ok(()),
        ResponseStatus::Error => Err(io::Error::other(res.payload.join("; "))),
    }
}

struct Stats {
    min: Duration,
    max: Duration,
    mean: Duration,
    p99: Duration,
}

fn compute_stats(latencies: &mut [Duration]) -> Stats {
    latencies.sort();
    let min = latencies[0];
    let max = *latencies.last().unwrap();
    let total: Duration = latencies.iter().sum();
    let mean = total / latencies.len() as u32;
    let p99_idx = (latencies.len() * 99 / 100).min(latencies.len() - 1);
    let p99 = latencies[p99_idx];
    Stats {
        min,
        max,
        mean,
        p99,
    }
}

fn print_phase(label: &str, count: usize, elapsed: Duration, stats: &Stats) {
    let throughput = count as f64 / elapsed.as_secs_f64();
    println!("=== {label} ({count} ops) ===");
    println!("  Total:      {elapsed:.3?}");
    println!("  Throughput: {throughput:.0} ops/sec");
    println!(
        "  Latency     min={:.3?}  mean={:.3?}  p99={:.3?}  max={:.3?}",
        stats.min, stats.mean, stats.p99, stats.max
    );
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let mut args_iter = args.iter().skip(1);
    let mut host = String::from("127.0.0.1:6666");
    let mut count: usize = 10_000;
    let mut value_size: usize = 100;

    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "-h" | "--host" => {
                if let Some(v) = args_iter.next() {
                    let addr: SocketAddr = v.parse().expect("Invalid TCP address");
                    host = addr.to_string();
                }
            }
            "-n" | "--count" => {
                if let Some(v) = args_iter.next() {
                    count = v.parse().expect("Invalid count (expected integer)");
                }
            }
            "-s" | "--value-size" => {
                if let Some(v) = args_iter.next() {
                    value_size = v.parse().expect("Invalid value size (expected integer)");
                }
            }
            "--help" => {
                println!("Usage: kvbench [OPTIONS]");
                println!();
                println!("Options:");
                println!("  -h, --host <addr>        Server address (default: 127.0.0.1:6666)");
                println!(
                    "  -n, --count <n>          Number of keys to write/read (default: 10000)"
                );
                println!("  -s, --value-size <bytes> Value size in bytes (default: 100)");
                return Ok(());
            }
            other => eprintln!("Unknown argument: {other}"),
        }
    }

    assert!(count > 0, "count must be > 0");
    assert!(value_size > 0, "value-size must be > 0");

    let value: String = "x".repeat(value_size);
    let keys: Vec<String> = (0..count).map(|i| format!("bench:key:{i:08}")).collect();

    let mut stream = TcpStream::connect(&host)?;
    println!("Connected to {host}");
    println!("Keys: {count}  Value size: {value_size} bytes");
    println!();

    // --- Write phase ---
    let mut write_latencies = Vec::with_capacity(count);
    let write_start = Instant::now();
    for key in &keys {
        let t = Instant::now();
        send_command(&mut stream, Command::Write(key.clone(), value.clone()))?;
        write_latencies.push(t.elapsed());
    }
    let write_elapsed = write_start.elapsed();

    // --- Read phase ---
    let mut read_latencies = Vec::with_capacity(count);
    let read_start = Instant::now();
    for key in &keys {
        let t = Instant::now();
        send_command(&mut stream, Command::Read(key.clone()))?;
        read_latencies.push(t.elapsed());
    }
    let read_elapsed = read_start.elapsed();

    // --- Print results ---
    print_phase(
        "WRITE",
        count,
        write_elapsed,
        &compute_stats(&mut write_latencies),
    );
    println!();
    print_phase(
        "READ",
        count,
        read_elapsed,
        &compute_stats(&mut read_latencies),
    );

    Ok(())
}
