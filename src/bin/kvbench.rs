//! # kvbench — Load generator and benchmark tool for rustikv
//!
//! Connects to a running rustikv TCP server and runs configurable workload
//! scenarios, measuring throughput and latency (min / mean / p99 / max).
//!
//! ## Modes
//!
//! | Mode | Description |
//! |------|-------------|
//! | `sequential` | Single connection, phases run one after another: WRITE → READ → RANGE → DELETE → OVERWRITE → post-overwrite READ |
//! | `concurrent` | Multi-threaded writers + readers in parallel, then sequential RANGE, DELETE, and OVERWRITE phases |
//! | `mixed` | Writers and readers hit an *overlapping* keyspace simultaneously — measures lock contention and snapshot isolation |
//!
//! ## Key distribution
//!
//! By default reads are uniform across the keyspace. Pass `--zipf <s>` to
//! switch to a Zipfian distribution where a small fraction of "hot" keys
//! receives the majority of reads (e.g. `--zipf 1.0` is classic Zipf's law).
//! This reveals whether Bloom filters and caching help under skewed access.
//!
//! ## Phases
//!
//! 1. **WRITE** — SET `count` keys with fixed-size values.
//! 2. **READ** — GET the same keys (uniform or Zipfian), with configurable `--miss-ratio`.
//! 3. **RANGE** — Sliding-window range queries across the sorted keyspace.
//! 4. **DELETE** — DELETE a configurable fraction of keys, then re-read to verify tombstones.
//! 5. **OVERWRITE** — Overwrite every surviving key N times, then re-read.

use std::{
    env,
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{
        Arc, Barrier,
        atomic::{AtomicBool, Ordering},
    },
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
/// Builds a list of (start, end) range queries that slide across the keyspace
/// in steps of `window` keys.
fn build_range_queries(keys: &[String], window: usize) -> Vec<(String, String)> {
    if keys.is_empty() || window == 0 {
        return vec![];
    }
    keys.windows(window)
        .step_by(window)
        .map(|w| (w[0].clone(), w[w.len() - 1].clone()))
        .collect()
}

/// Simple xorshift64 PRNG — deterministic, no external crate needed.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Returns a float in [0, 1).
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }

    /// Generates a Zipfian-distributed index in [0, n) using rejection-inversion.
    /// `s` is the skew exponent (1.0 = classic Zipf).
    fn next_zipf(&mut self, n: usize, s: f64) -> usize {
        // Zipf CDF inversion approximation (good enough for benchmarks)
        let u = self.next_f64();
        let n_f = n as f64;
        if s == 1.0 {
            // H(n) ≈ ln(n) + 0.5772
            let h_n = n_f.ln() + 0.5772;
            let x = (u * h_n).exp();
            (x as usize).min(n - 1)
        } else {
            // Generalised harmonic: H(n,s) ≈ ∫₁ⁿ x^{-s} dx = (n^{1-s} - 1)/(1-s)
            let exp = 1.0 - s;
            let h_n = (n_f.powf(exp) - 1.0) / exp;
            let x = (u * h_n * exp + 1.0).powf(1.0 / exp);
            (x as usize).min(n - 1)
        }
    }
}

/// Builds a read-key sequence with Zipfian distribution.
/// Hot keys (low rank) get the majority of reads.
fn build_zipf_read_keys(keys: &[String], count: usize, s: f64, miss_ratio: f64) -> Vec<String> {
    let n = keys.len();
    let miss_count = (count as f64 * miss_ratio).round() as usize;
    let mut rng = Rng::new(0xDEAD_BEEF_CAFE);
    (0..count)
        .map(|i| {
            if miss_count > 0 && (i * miss_count) / count < ((i + 1) * miss_count) / count {
                format!("bench:missing:{i:08}")
            } else {
                let idx = rng.next_zipf(n, s);
                keys[idx].clone()
            }
        })
        .collect()
}

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

struct BenchConfig {
    host: String,
    count: usize,
    value_size: usize,
    miss_ratio: f64,
    range_window: usize,
    delete_ratio: f64,
    overwrite_rounds: usize,
    zipf_s: Option<f64>,
    n_writers: usize,
    n_readers: usize,
    mixed_duration: u64,
}

fn sequential(cfg: &BenchConfig) -> io::Result<()> {
    let BenchConfig {
        ref host,
        count,
        value_size,
        miss_ratio,
        range_window,
        delete_ratio,
        overwrite_rounds,
        zipf_s,
        ..
    } = *cfg;
    println!(
        "Mode: sequential  Keys: {count}  Value size: {value_size}B  Miss ratio: {:.0}%  Range window: {range_window}  Delete ratio: {:.0}%  Overwrite rounds: {overwrite_rounds}  Distribution: {}\n",
        miss_ratio * 100.0,
        delete_ratio * 100.0,
        zipf_s.map_or("uniform".to_string(), |s| format!("zipf(s={s})")),
    );

    let value: String = "x".repeat(value_size);
    let keys: Vec<String> = (0..count).map(|i| format!("bench:key:{i:08}")).collect();
    let read_keys = match zipf_s {
        Some(s) => build_zipf_read_keys(&keys, count, s, miss_ratio),
        None => build_read_keys(&keys, miss_ratio),
    };

    let mut stream = TcpStream::connect(host)?;

    // ── WRITE phase ──
    let mut write_latencies = Vec::with_capacity(count);
    let write_start = Instant::now();
    for key in &keys {
        let t = Instant::now();
        send_command(&mut stream, Command::Write(key.clone(), value.clone()))?;
        write_latencies.push(t.elapsed());
    }
    let write_elapsed = write_start.elapsed();

    // ── READ phase ──
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

    // ── RANGE phase ──
    let range_queries = build_range_queries(&keys, range_window);
    let mut range_latencies = Vec::with_capacity(range_queries.len());
    let range_start = Instant::now();
    for (start, end) in &range_queries {
        let t = Instant::now();
        send_command(&mut stream, Command::Range(start.clone(), end.clone()))?;
        range_latencies.push(t.elapsed());
    }
    let range_elapsed = range_start.elapsed();

    // ── DELETE phase ──
    let delete_count = (count as f64 * delete_ratio).round() as usize;
    let delete_keys: Vec<&String> = keys
        .iter()
        .step_by(
            if delete_count == 0 {
                1
            } else {
                count / delete_count.max(1)
            }
            .max(1),
        )
        .take(delete_count)
        .collect();
    let mut delete_latencies = Vec::with_capacity(delete_count);
    let delete_start = Instant::now();
    for key in &delete_keys {
        let t = Instant::now();
        send_command(&mut stream, Command::Delete((*key).clone()))?;
        delete_latencies.push(t.elapsed());
    }
    let delete_elapsed = delete_start.elapsed();

    // Re-read after delete to measure tombstone impact
    let mut post_del_latencies = Vec::with_capacity(count);
    let mut post_del_misses = 0usize;
    let post_del_start = Instant::now();
    for key in &keys {
        let t = Instant::now();
        if let SendResult::Miss = send_command(&mut stream, Command::Read(key.clone()))? {
            post_del_misses += 1;
        }
        post_del_latencies.push(t.elapsed());
    }
    let post_del_elapsed = post_del_start.elapsed();

    // ── OVERWRITE phase ──
    let surviving_keys: Vec<&String> = keys.iter().filter(|k| !delete_keys.contains(k)).collect();
    let overwrite_value: String = "y".repeat(value_size);
    let mut overwrite_latencies = Vec::with_capacity(surviving_keys.len() * overwrite_rounds);
    let overwrite_start = Instant::now();
    for _round in 0..overwrite_rounds {
        for key in &surviving_keys {
            let t = Instant::now();
            send_command(
                &mut stream,
                Command::Write((*key).clone(), overwrite_value.clone()),
            )?;
            overwrite_latencies.push(t.elapsed());
        }
    }
    let overwrite_elapsed = overwrite_start.elapsed();

    // Re-read after overwrite
    let mut post_ow_latencies = Vec::with_capacity(surviving_keys.len());
    let mut post_ow_misses = 0usize;
    let post_ow_start = Instant::now();
    for key in &surviving_keys {
        let t = Instant::now();
        if let SendResult::Miss = send_command(&mut stream, Command::Read((*key).clone()))? {
            post_ow_misses += 1;
        }
        post_ow_latencies.push(t.elapsed());
    }
    let post_ow_elapsed = post_ow_start.elapsed();

    // ── Print results ──
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
        &format!(
            "READ ({})",
            zipf_s.map_or("uniform".to_string(), |s| format!("zipf s={s}"))
        ),
        &mut PhaseResult {
            latencies: read_latencies,
            misses,
            elapsed: read_elapsed,
        },
    );
    println!();
    print_phase(
        &format!("RANGE (window={range_window})"),
        &mut PhaseResult {
            latencies: range_latencies,
            misses: 0,
            elapsed: range_elapsed,
        },
    );
    if delete_count > 0 {
        println!();
        print_phase(
            &format!("DELETE ({delete_count} keys, {:.0}%)", delete_ratio * 100.0),
            &mut PhaseResult {
                latencies: delete_latencies,
                misses: 0,
                elapsed: delete_elapsed,
            },
        );
        println!();
        print_phase(
            "READ-AFTER-DELETE",
            &mut PhaseResult {
                latencies: post_del_latencies,
                misses: post_del_misses,
                elapsed: post_del_elapsed,
            },
        );
    }
    if overwrite_rounds > 0 {
        println!();
        print_phase(
            &format!("OVERWRITE ({overwrite_rounds}x)"),
            &mut PhaseResult {
                latencies: overwrite_latencies,
                misses: 0,
                elapsed: overwrite_elapsed,
            },
        );
        println!();
        print_phase(
            "READ-AFTER-OVERWRITE",
            &mut PhaseResult {
                latencies: post_ow_latencies,
                misses: post_ow_misses,
                elapsed: post_ow_elapsed,
            },
        );
    }

    Ok(())
}

fn concurrent(cfg: &BenchConfig) -> io::Result<()> {
    let BenchConfig {
        ref host,
        count,
        value_size,
        miss_ratio,
        n_writers,
        n_readers,
        range_window,
        delete_ratio,
        overwrite_rounds,
        zipf_s,
        ..
    } = *cfg;
    println!(
        "Mode: concurrent  Keys: {count}  Value size: {value_size}B  Writers: {n_writers}  Readers: {n_readers}  Miss ratio: {:.0}%  Range window: {range_window}  Delete ratio: {:.0}%  Overwrite rounds: {overwrite_rounds}  Distribution: {}\n",
        miss_ratio * 100.0,
        delete_ratio * 100.0,
        zipf_s.map_or("uniform".to_string(), |s| format!("zipf(s={s})")),
    );

    let value = Arc::new("x".repeat(value_size));
    let write_keys = Arc::new(
        (0..count)
            .map(|i| format!("bench:key:{i:08}"))
            .collect::<Vec<_>>(),
    );
    let read_keys = Arc::new(match zipf_s {
        Some(s) => build_zipf_read_keys(&write_keys, count, s, miss_ratio),
        None => build_read_keys(&write_keys, miss_ratio),
    });
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

    // Sequential range phase (after writers and readers finish)
    let range_queries = build_range_queries(&write_keys, range_window);
    let mut range_latencies = Vec::with_capacity(range_queries.len());
    let mut range_stream = TcpStream::connect(host.as_str())?;
    let range_start = Instant::now();
    for (start, end) in &range_queries {
        let t = Instant::now();
        send_command(
            &mut range_stream,
            Command::Range(start.clone(), end.clone()),
        )?;
        range_latencies.push(t.elapsed());
    }
    let range_elapsed = range_start.elapsed();

    // ── DELETE phase (sequential, after concurrent r/w) ──
    let delete_count = (count as f64 * delete_ratio).round() as usize;
    let delete_keys: Vec<String> = write_keys
        .iter()
        .step_by(
            if delete_count == 0 {
                1
            } else {
                count / delete_count.max(1)
            }
            .max(1),
        )
        .take(delete_count)
        .cloned()
        .collect();
    let mut delete_latencies = Vec::with_capacity(delete_count);
    let mut del_stream = TcpStream::connect(host.as_str())?;
    let delete_start = Instant::now();
    for key in &delete_keys {
        let t = Instant::now();
        send_command(&mut del_stream, Command::Delete(key.clone()))?;
        delete_latencies.push(t.elapsed());
    }
    let delete_elapsed = delete_start.elapsed();

    // Re-read after delete
    let mut post_del_latencies = Vec::with_capacity(count);
    let mut post_del_misses = 0usize;
    let post_del_start = Instant::now();
    for key in write_keys.iter() {
        let t = Instant::now();
        if let SendResult::Miss = send_command(&mut del_stream, Command::Read(key.clone()))? {
            post_del_misses += 1;
        }
        post_del_latencies.push(t.elapsed());
    }
    let post_del_elapsed = post_del_start.elapsed();

    // ── OVERWRITE phase (sequential, after delete) ──
    let surviving_keys: Vec<&String> = write_keys
        .iter()
        .filter(|k| !delete_keys.contains(k))
        .collect();
    let overwrite_value: String = "y".repeat(value_size);
    let mut overwrite_latencies = Vec::with_capacity(surviving_keys.len() * overwrite_rounds);
    let overwrite_start = Instant::now();
    for _round in 0..overwrite_rounds {
        for key in &surviving_keys {
            let t = Instant::now();
            send_command(
                &mut del_stream,
                Command::Write((*key).clone(), overwrite_value.clone()),
            )?;
            overwrite_latencies.push(t.elapsed());
        }
    }
    let overwrite_elapsed = overwrite_start.elapsed();

    // Re-read after overwrite
    let mut post_ow_latencies = Vec::with_capacity(surviving_keys.len());
    let mut post_ow_misses = 0usize;
    let post_ow_start = Instant::now();
    for key in &surviving_keys {
        let t = Instant::now();
        if let SendResult::Miss = send_command(&mut del_stream, Command::Read((*key).clone()))? {
            post_ow_misses += 1;
        }
        post_ow_latencies.push(t.elapsed());
    }
    let post_ow_elapsed = post_ow_start.elapsed();

    let total_ops = all_write.latencies.len() + all_read.latencies.len();
    let wall = all_write.elapsed.max(all_read.elapsed);
    let agg_throughput = total_ops as f64 / wall.as_secs_f64();

    print_phase("WRITE", &mut all_write);
    println!();
    print_phase("READ", &mut all_read);
    println!();
    print_phase(
        &format!("RANGE (window={range_window})"),
        &mut PhaseResult {
            latencies: range_latencies,
            misses: 0,
            elapsed: range_elapsed,
        },
    );
    if delete_count > 0 {
        println!();
        print_phase(
            &format!("DELETE ({delete_count} keys, {:.0}%)", delete_ratio * 100.0),
            &mut PhaseResult {
                latencies: delete_latencies,
                misses: 0,
                elapsed: delete_elapsed,
            },
        );
        println!();
        print_phase(
            "READ-AFTER-DELETE",
            &mut PhaseResult {
                latencies: post_del_latencies,
                misses: post_del_misses,
                elapsed: post_del_elapsed,
            },
        );
    }
    if overwrite_rounds > 0 {
        println!();
        print_phase(
            &format!("OVERWRITE ({overwrite_rounds}x)"),
            &mut PhaseResult {
                latencies: overwrite_latencies,
                misses: 0,
                elapsed: overwrite_elapsed,
            },
        );
        println!();
        print_phase(
            "READ-AFTER-OVERWRITE",
            &mut PhaseResult {
                latencies: post_ow_latencies,
                misses: post_ow_misses,
                elapsed: post_ow_elapsed,
            },
        );
    }
    println!();
    println!("=== AGGREGATE ({total_ops} ops) ===");
    println!("  Wall time:  {wall:.3?}");
    println!("  Throughput: {agg_throughput:.0} ops/sec");

    Ok(())
}

/// Mixed concurrent mode: writers and readers hit an *overlapping* keyspace
/// simultaneously. Unlike `concurrent` mode (which writes first, then reads),
/// this mode starts both at the same time on the same keys, exposing lock
/// contention on the memtable / hash-index and testing snapshot isolation.
fn mixed(cfg: &BenchConfig) -> io::Result<()> {
    let BenchConfig {
        ref host,
        count,
        value_size,
        n_writers,
        n_readers,
        mixed_duration: duration_secs,
        ..
    } = *cfg;
    println!(
        "Mode: mixed  Keys: {count}  Value size: {value_size}B  Writers: {n_writers}  Readers: {n_readers}  Duration: {duration_secs}s\n"
    );

    let value = Arc::new("x".repeat(value_size));
    let keys = Arc::new(
        (0..count)
            .map(|i| format!("bench:key:{i:08}"))
            .collect::<Vec<_>>(),
    );
    let host = Arc::new(host.to_string());
    let stop = Arc::new(AtomicBool::new(false));
    let barrier = Arc::new(Barrier::new(n_writers + n_readers));

    // Spawn writer threads — loop over the keyspace writing until `stop` is set
    let mut write_handles = Vec::new();
    for _w in 0..n_writers {
        let host = Arc::clone(&host);
        let keys = Arc::clone(&keys);
        let value = Arc::clone(&value);
        let stop = Arc::clone(&stop);
        let barrier = Arc::clone(&barrier);
        write_handles.push(thread::spawn(move || -> io::Result<PhaseResult> {
            let mut stream = TcpStream::connect(host.as_str())?;
            barrier.wait();
            let mut latencies = Vec::new();
            let phase_start = Instant::now();
            let mut idx = 0;
            while !stop.load(Ordering::Relaxed) {
                let key = &keys[idx % keys.len()];
                let t = Instant::now();
                send_command(&mut stream, Command::Write(key.clone(), (*value).clone()))?;
                latencies.push(t.elapsed());
                idx += 1;
            }
            Ok(PhaseResult {
                latencies,
                misses: 0,
                elapsed: phase_start.elapsed(),
            })
        }));
    }

    // Spawn reader threads — loop over the keyspace reading until `stop` is set
    let mut read_handles = Vec::new();
    for _r in 0..n_readers {
        let host = Arc::clone(&host);
        let keys = Arc::clone(&keys);
        let stop = Arc::clone(&stop);
        let barrier = Arc::clone(&barrier);
        read_handles.push(thread::spawn(move || -> io::Result<PhaseResult> {
            let mut stream = TcpStream::connect(host.as_str())?;
            barrier.wait();
            let mut latencies = Vec::new();
            let mut misses = 0usize;
            let phase_start = Instant::now();
            let mut idx = 0;
            while !stop.load(Ordering::Relaxed) {
                let key = &keys[idx % keys.len()];
                let t = Instant::now();
                if let SendResult::Miss = send_command(&mut stream, Command::Read(key.clone()))? {
                    misses += 1;
                }
                latencies.push(t.elapsed());
                idx += 1;
            }
            Ok(PhaseResult {
                latencies,
                misses,
                elapsed: phase_start.elapsed(),
            })
        }));
    }

    // Let it run for the requested duration
    let wall_start = Instant::now();
    loop {
        thread::yield_now();
        if wall_start.elapsed() >= Duration::from_secs(duration_secs) {
            stop.store(true, Ordering::Relaxed);
            break;
        }
    }

    // Collect results
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

    print_phase("WRITE (mixed)", &mut all_write);
    println!();
    print_phase("READ (mixed)", &mut all_read);
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
    let mut range_window: usize = 100;
    let mut delete_ratio: f64 = 0.0;
    let mut overwrite_rounds: usize = 0;
    let mut zipf_s: Option<f64> = None;
    let mut mixed_duration: u64 = 10;

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
            "-r" | "--range-window" => {
                if let Some(v) = args_iter.next() {
                    range_window = v.parse().expect("Invalid range window (expected integer)");
                }
            }
            "-d" | "--delete-ratio" => {
                if let Some(v) = args_iter.next() {
                    delete_ratio = v.parse().expect("Invalid delete ratio (expected 0.0–1.0)");
                    assert!(
                        (0.0..=1.0).contains(&delete_ratio),
                        "delete-ratio must be between 0.0 and 1.0"
                    );
                }
            }
            "-o" | "--overwrite-rounds" => {
                if let Some(v) = args_iter.next() {
                    overwrite_rounds = v
                        .parse()
                        .expect("Invalid overwrite rounds (expected integer)");
                }
            }
            "--zipf" => {
                if let Some(v) = args_iter.next() {
                    let s: f64 = v
                        .parse()
                        .expect("Invalid zipf exponent (expected float > 0)");
                    assert!(s > 0.0, "zipf exponent must be > 0");
                    zipf_s = Some(s);
                }
            }
            "--mixed-duration" => {
                if let Some(v) = args_iter.next() {
                    mixed_duration = v
                        .parse()
                        .expect("Invalid mixed duration (expected seconds)");
                }
            }
            "--help" => {
                println!("Usage: kvbench [OPTIONS]");
                println!();
                println!("Options:");
                println!("  -h, --host <addr>           Server address (default: 127.0.0.1:6666)");
                println!("  -n, --count <n>             Number of keys (default: 10000)");
                println!("  -s, --value-size <bytes>    Value size in bytes (default: 100)");
                println!(
                    "  -m, --miss-ratio <ratio>    Fraction of reads targeting missing keys, 0.0–1.0 (default: 0.0)"
                );
                println!(
                    "      --mode <mode>           sequential|concurrent|mixed (default: sequential)"
                );
                println!(
                    "      --writers <n>           Writer threads, concurrent/mixed mode (default: 4)"
                );
                println!(
                    "      --readers <n>           Reader threads, concurrent/mixed mode (default: 4)"
                );
                println!(
                    "  -r, --range-window <n>      Keys per range query window (default: 100)"
                );
                println!(
                    "  -d, --delete-ratio <ratio>  Fraction of keys to delete, 0.0–1.0 (default: 0.0)"
                );
                println!(
                    "  -o, --overwrite-rounds <n>  Number of overwrite passes over surviving keys (default: 0)"
                );
                println!(
                    "      --zipf <s>              Use Zipfian read distribution with exponent s (e.g. 1.0)"
                );
                println!(
                    "      --mixed-duration <secs> Duration for mixed mode in seconds (default: 10)"
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

    let cfg = BenchConfig {
        host,
        count,
        value_size,
        miss_ratio,
        range_window,
        delete_ratio,
        overwrite_rounds,
        zipf_s,
        n_writers,
        n_readers,
        mixed_duration,
    };

    match mode.as_str() {
        "sequential" => sequential(&cfg),
        "concurrent" => concurrent(&cfg),
        "mixed" => mixed(&cfg),
        other => {
            eprintln!("Unknown mode: {other}. Use 'sequential', 'concurrent', or 'mixed'.");
            std::process::exit(1);
        }
    }
}
