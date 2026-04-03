use std::{
    env,
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use rustikv::bffp::{Command, ResponseStatus, decode_response_frame, encode_command};

enum SendResult {
    Hit,
    Miss,
}

fn send_command(stream: &mut TcpStream, cmd: Command) -> io::Result<SendResult> {
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
        ResponseStatus::Ok | ResponseStatus::Noop => Ok(SendResult::Hit),
        ResponseStatus::NotFound => Ok(SendResult::Miss),
        ResponseStatus::Error => Err(io::Error::other(res.payload.join("; "))),
    }
}

struct PhaseResult {
    latencies: Vec<Duration>,
    misses: usize,
    elapsed: Duration,
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

fn print_phase(label: &str, result: &mut PhaseResult) {
    let count = result.latencies.len();
    if count == 0 {
        return;
    }
    let throughput = count as f64 / result.elapsed.as_secs_f64();
    let stats = compute_stats(&mut result.latencies);
    println!("=== {label} ({count} ops) ===");
    println!("  Total:      {:.3?}", result.elapsed);
    println!("  Throughput: {throughput:.0} ops/sec");
    println!(
        "  Latency     min={:.3?}  mean={:.3?}  p99={:.3?}  max={:.3?}",
        stats.min, stats.mean, stats.p99, stats.max
    );
    if result.misses > 0 {
        let miss_pct = result.misses as f64 / count as f64 * 100.0;
        println!("  Misses:     {} ({miss_pct:.1}%)", result.misses);
    }
}

// Spreads `miss_ratio` fraction of keys evenly across the read set,
// replacing them with keys that were never written.
fn build_read_keys(keys: &[String], miss_ratio: f64) -> Vec<String> {
    let n = keys.len();
    let miss_count = (n as f64 * miss_ratio).round() as usize;
    keys.iter()
        .enumerate()
        .map(|(i, k)| {
            // Bresenham-style: emit a miss whenever the running total crosses an integer
            if miss_count > 0 && (i * miss_count) / n < ((i + 1) * miss_count) / n {
                format!("bench:missing:{i:08}")
            } else {
                k.clone()
            }
        })
        .collect()
}

fn sequential(host: &str, count: usize, value_size: usize, miss_ratio: f64) -> io::Result<()> {
    println!(
        "Mode: sequential  Keys: {count}  Value size: {value_size}B  Miss ratio: {:.0}%\n",
        miss_ratio * 100.0
    );

    let value: String = "x".repeat(value_size);
    let keys: Vec<String> = (0..count).map(|i| format!("bench:key:{i:08}")).collect();
    let read_keys = build_read_keys(&keys, miss_ratio);

    let mut stream = TcpStream::connect(host)?;

    // Write phase
    let mut write_latencies = Vec::with_capacity(count);
    let write_start = Instant::now();
    for key in &keys {
        let t = Instant::now();
        send_command(&mut stream, Command::Write(key.clone(), value.clone()))?;
        write_latencies.push(t.elapsed());
    }
    let write_elapsed = write_start.elapsed();

    // Read phase
    let mut read_latencies = Vec::with_capacity(count);
    let mut misses = 0usize;
    let read_start = Instant::now();
    for key in &read_keys {
        let t = Instant::now();
        if let SendResult::Miss = send_command(&mut stream, Command::Read(key.clone()))? {
            misses += 1;
        }
        read_latencies.push(t.elapsed());
    }
    let read_elapsed = read_start.elapsed();

    print_phase(
        "WRITE",
        &mut PhaseResult {
            latencies: write_latencies,
            misses: 0,
            elapsed: write_elapsed,
        },
    );
    println!();
    print_phase(
        "READ",
        &mut PhaseResult {
            latencies: read_latencies,
            misses,
            elapsed: read_elapsed,
        },
    );

    Ok(())
}

fn concurrent(
    host: &str,
    count: usize,
    value_size: usize,
    miss_ratio: f64,
    n_writers: usize,
    n_readers: usize,
) -> io::Result<()> {
    println!(
        "Mode: concurrent  Keys: {count}  Value size: {value_size}B  Writers: {n_writers}  Readers: {n_readers}  Miss ratio: {:.0}%\n",
        miss_ratio * 100.0
    );

    let value = Arc::new("x".repeat(value_size));
    let write_keys = Arc::new(
        (0..count)
            .map(|i| format!("bench:key:{i:08}"))
            .collect::<Vec<_>>(),
    );
    let read_keys = Arc::new(build_read_keys(&write_keys, miss_ratio));
    let host = Arc::new(host.to_string());
    let barrier = Arc::new(Barrier::new(n_writers + n_readers));

    // Spawn writer threads
    let mut write_handles = Vec::new();
    for w in 0..n_writers {
        let host = Arc::clone(&host);
        let keys = Arc::clone(&write_keys);
        let value = Arc::clone(&value);
        let barrier = Arc::clone(&barrier);
        let start = w * count / n_writers;
        let end = if w + 1 == n_writers {
            count
        } else {
            (w + 1) * count / n_writers
        };

        write_handles.push(thread::spawn(move || -> io::Result<PhaseResult> {
            let mut stream = TcpStream::connect(host.as_str())?;
            barrier.wait();
            let mut latencies = Vec::with_capacity(end - start);
            let phase_start = Instant::now();
            for key in &keys[start..end] {
                let t = Instant::now();
                send_command(&mut stream, Command::Write(key.clone(), (*value).clone()))?;
                latencies.push(t.elapsed());
            }
            Ok(PhaseResult {
                latencies,
                misses: 0,
                elapsed: phase_start.elapsed(),
            })
        }));
    }

    // Spawn reader threads
    let rcount = read_keys.len();
    let mut read_handles = Vec::new();
    for r in 0..n_readers {
        let host = Arc::clone(&host);
        let keys = Arc::clone(&read_keys);
        let barrier = Arc::clone(&barrier);
        let start = r * rcount / n_readers;
        let end = if r + 1 == n_readers {
            rcount
        } else {
            (r + 1) * rcount / n_readers
        };

        read_handles.push(thread::spawn(move || -> io::Result<PhaseResult> {
            let mut stream = TcpStream::connect(host.as_str())?;
            barrier.wait();
            let mut latencies = Vec::with_capacity(end - start);
            let mut misses = 0usize;
            let phase_start = Instant::now();
            for key in &keys[start..end] {
                let t = Instant::now();
                if let SendResult::Miss = send_command(&mut stream, Command::Read(key.clone()))? {
                    misses += 1;
                }
                latencies.push(t.elapsed());
            }
            Ok(PhaseResult {
                latencies,
                misses,
                elapsed: phase_start.elapsed(),
            })
        }));
    }

    // Collect write results (wall time = max thread elapsed, since all started at the barrier)
    let mut all_write = PhaseResult {
        latencies: Vec::new(),
        misses: 0,
        elapsed: Duration::ZERO,
    };
    for h in write_handles {
        let mut r = h.join().expect("writer thread panicked")?;
        if r.elapsed > all_write.elapsed {
            all_write.elapsed = r.elapsed;
        }
        all_write.latencies.append(&mut r.latencies);
    }

    // Collect read results
    let mut all_read = PhaseResult {
        latencies: Vec::new(),
        misses: 0,
        elapsed: Duration::ZERO,
    };
    for h in read_handles {
        let mut r = h.join().expect("reader thread panicked")?;
        if r.elapsed > all_read.elapsed {
            all_read.elapsed = r.elapsed;
        }
        all_read.latencies.append(&mut r.latencies);
        all_read.misses += r.misses;
    }

    let total_ops = all_write.latencies.len() + all_read.latencies.len();
    let wall = all_write.elapsed.max(all_read.elapsed);
    let agg_throughput = total_ops as f64 / wall.as_secs_f64();

    print_phase("WRITE", &mut all_write);
    println!();
    print_phase("READ", &mut all_read);
    println!();
    println!("=== AGGREGATE ({total_ops} ops) ===");
    println!("  Wall time:  {wall:.3?}");
    println!("  Throughput: {agg_throughput:.0} ops/sec");

    Ok(())
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let mut args_iter = args.iter().skip(1);
    let mut host = String::from("127.0.0.1:6666");
    let mut count: usize = 10_000;
    let mut value_size: usize = 100;
    let mut miss_ratio: f64 = 0.0;
    let mut mode = String::from("sequential");
    let mut n_writers: usize = 4;
    let mut n_readers: usize = 4;

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
            "-m" | "--miss-ratio" => {
                if let Some(v) = args_iter.next() {
                    miss_ratio = v.parse().expect("Invalid miss ratio (expected 0.0–1.0)");
                    assert!(
                        (0.0..=1.0).contains(&miss_ratio),
                        "miss-ratio must be between 0.0 and 1.0"
                    );
                }
            }
            "--mode" => {
                if let Some(v) = args_iter.next() {
                    mode = v.to_string();
                }
            }
            "--writers" => {
                if let Some(v) = args_iter.next() {
                    n_writers = v.parse().expect("Invalid writers count (expected integer)");
                }
            }
            "--readers" => {
                if let Some(v) = args_iter.next() {
                    n_readers = v.parse().expect("Invalid readers count (expected integer)");
                }
            }
            "--help" => {
                println!("Usage: kvbench [OPTIONS]");
                println!();
                println!("Options:");
                println!("  -h, --host <addr>        Server address (default: 127.0.0.1:6666)");
                println!("  -n, --count <n>          Number of keys (default: 10000)");
                println!("  -s, --value-size <bytes> Value size in bytes (default: 100)");
                println!(
                    "  -m, --miss-ratio <ratio> Fraction of reads targeting missing keys, 0.0–1.0 (default: 0.0)"
                );
                println!("      --mode <mode>        sequential|concurrent (default: sequential)");
                println!(
                    "      --writers <n>        Writer threads, concurrent mode only (default: 4)"
                );
                println!(
                    "      --readers <n>        Reader threads, concurrent mode only (default: 4)"
                );
                return Ok(());
            }
            other => eprintln!("Unknown argument: {other}"),
        }
    }

    assert!(count > 0, "count must be > 0");
    assert!(value_size > 0, "value-size must be > 0");
    assert!(n_writers > 0, "writers must be > 0");
    assert!(n_readers > 0, "readers must be > 0");

    println!("Connected to {host}");

    match mode.as_str() {
        "sequential" => sequential(&host, count, value_size, miss_ratio),
        "concurrent" => concurrent(&host, count, value_size, miss_ratio, n_writers, n_readers),
        other => {
            eprintln!("Unknown mode: {other}. Use 'sequential' or 'concurrent'.");
            std::process::exit(1);
        }
    }
}
