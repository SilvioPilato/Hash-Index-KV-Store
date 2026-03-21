use hash_index::engine::StorageEngine;
use hash_index::kvengine::KVEngine;
use hash_index::lsmengine::LsmEngine;
use hash_index::record::{MAX_KEY_SIZE, MAX_VALUE_SIZE};
use hash_index::settings::{EngineType, Settings};
use hash_index::stats::Stats;
use std::env;
use std::io::{self};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, RwLock, atomic::Ordering},
    thread,
    time::Instant,
};

enum Command {
    Write(String, String),
    Read(String),
    Delete(String),
    Invalid(String),
    Compact,
    Stats,
}

fn verbose_logging_enabled() -> bool {
    matches!(env::var("KV_STORE_VERBOSE"), Ok(value) if value == "1")
}

fn log_verbose(message: impl AsRef<str>) {
    if verbose_logging_enabled() {
        println!("{}", message.as_ref());
    }
}

fn main() -> io::Result<()> {
    let settings = Settings::get_from_args();

    let database: Box<dyn StorageEngine> = match settings.engine {
        EngineType::KV => match KVEngine::from_dir(
            &settings.db_file_path,
            &settings.db_name,
            settings.max_segment_bytes,
            settings.sync_strategy,
        )? {
            Some(db) => Box::new(db),
            None => Box::new(KVEngine::new(
                &settings.db_file_path,
                &settings.db_name,
                settings.max_segment_bytes,
                settings.sync_strategy,
            )?),
        },
        EngineType::Lsm => Box::new(LsmEngine::from_dir(
            &settings.db_file_path,
            &settings.db_name,
            settings.max_segment_bytes as usize,
        )?),
    };

    let db_handle: Arc<RwLock<Box<dyn StorageEngine>>> = Arc::new(RwLock::new(database));
    let stats = Arc::new(Stats::new());
    let listener = TcpListener::bind(&settings.tcp_addr)?;
    let actual_addr = listener.local_addr()?;

    // Convert 0.0.0.0 to 127.0.0.1 for client connections
    let connect_addr = match actual_addr {
        std::net::SocketAddr::V4(addr) if addr.ip().is_unspecified() => std::net::SocketAddr::V4(
            std::net::SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, addr.port()),
        ),
        std::net::SocketAddr::V6(addr) if addr.ip().is_unspecified() => std::net::SocketAddr::V6(
            std::net::SocketAddrV6::new(std::net::Ipv6Addr::LOCALHOST, addr.port(), 0, 0),
        ),
        _ => actual_addr,
    };

    let addr_file = format!("{}/server.addr", &settings.db_file_path);
    std::fs::write(&addr_file, connect_addr.to_string())?;

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                log_verbose(format!("New connection: {}", stream.peer_addr().unwrap()));
                let shared_db = Arc::clone(&db_handle);
                let shared_stats = Arc::clone(&stats);
                thread::spawn(move || {
                    shared_stats
                        .active_connections
                        .fetch_add(1, Ordering::Relaxed);
                    let response = handle_stream(
                        &stream,
                        shared_db,
                        &shared_stats,
                        settings.compaction_ratio,
                        settings.compaction_max_segment,
                    );
                    shared_stats
                        .active_connections
                        .fetch_sub(1, Ordering::Relaxed);
                    stream
                        .write_all(response.as_bytes())
                        .expect("Could not write response to stream");
                });
            }
            Err(e) => {
                eprintln!("Error accepting connection: {}", e);
            }
        }
    }
    Ok(())
}

fn handle_stream(
    stream: &TcpStream,
    database: Arc<RwLock<Box<dyn StorageEngine>>>,
    stats: &Arc<Stats>,
    compaction_ratio: f32,
    compaction_max_segment: usize,
) -> String {
    const MAX_REQUEST_BYTES: u64 = (MAX_KEY_SIZE + MAX_VALUE_SIZE + 1024) as u64;
    let reader = BufReader::new(stream.take(MAX_REQUEST_BYTES));
    let request: Vec<_> = reader
        .lines()
        .map(|result| result.unwrap_or_else(|_| String::new()))
        .take_while(|line| !line.is_empty())
        .collect();

    if request.is_empty() {
        return "Received empty request.".to_string();
    }

    match parse_message(&request.concat()) {
        Command::Write(key, value) => {
            log_verbose(format!(
                "Parsed WRITE command: key='{}', value='{}'",
                key, value
            ));
            if stats.compacting.load(Ordering::Relaxed) {
                stats.write_blocked_attempts.fetch_add(1, Ordering::Relaxed);
            }
            let lock_start = Instant::now();
            let result = {
                let mut db = database.write().unwrap();
                db.set(&key, &value)
            };
            let lock_elapsed = lock_start.elapsed().as_millis() as u64;
            stats
                .write_blocked_total_ms
                .fetch_add(lock_elapsed, Ordering::Relaxed);
            match result {
                Ok(_) => {
                    stats.writes.fetch_add(1, Ordering::Relaxed);
                    maybe_trigger_compaction(
                        database.clone(),
                        stats,
                        compaction_ratio,
                        compaction_max_segment,
                    );
                    "OK".to_string()
                }
                Err(err) => err.to_string(),
            }
        }
        Command::Read(key) => {
            log_verbose(format!("Parsed READ command: key='{}'", key));
            let db = database.read().unwrap();
            stats.reads.fetch_add(1, Ordering::Relaxed);
            match db.get(&key) {
                Ok(result) => match result {
                    Some((_, v)) => v,
                    None => "Not found".to_string(),
                },
                Err(error) => error.to_string(),
            }
        }
        Command::Delete(key) => {
            log_verbose(format!("Parsed DELETE command: key='{}'", key));
            let result = {
                let mut db = database.write().unwrap();
                db.delete(&key)
            };
            match result {
                Ok(Some(())) => {
                    stats.deletes.fetch_add(1, Ordering::Relaxed);
                    maybe_trigger_compaction(
                        database.clone(),
                        stats,
                        compaction_ratio,
                        compaction_max_segment,
                    );
                    "OK".to_string()
                }
                Ok(None) => "Not found".to_string(),
                Err(error) => error.to_string(),
            }
        }
        Command::Compact => {
            log_verbose("Parsed COMPACT command");
            if stats
                .compacting
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return "NOOP".to_string();
            }
            stats
                .last_compact_start_ms
                .store(Stats::now_ms(), Ordering::Relaxed);
            let db_clone = Arc::clone(&database);
            let stats_clone = Arc::clone(stats);
            thread::spawn(move || {
                let mut db = db_clone.write().unwrap();
                db.compact().unwrap();

                stats_clone
                    .last_compact_end_ms
                    .store(Stats::now_ms(), Ordering::Relaxed);
                stats_clone.compacting.store(false, Ordering::Release);
                stats_clone.compaction_count.fetch_add(1, Ordering::Relaxed);
            });
            "OK".to_string()
        }
        Command::Stats => stats.snapshot(),
        Command::Invalid(original_command) => {
            log_verbose(format!("Invalid command: {}", original_command));
            format!("Invalid command: {}", original_command)
        }
    }
}

fn parse_message(message: &str) -> Command {
    let words: Vec<&str> = message.split_whitespace().collect();

    if words.is_empty() {
        return Command::Invalid(message.to_string());
    }

    match words[0].to_uppercase().as_str() {
        "WRITE" => {
            let rest = message["WRITE".len()..].trim_start();
            if let Some(i) = rest.find(char::is_whitespace) {
                let key = &rest[..i];
                let value_start = i + rest[i..].char_indices().nth(1).map_or(0, |(j, _)| j);
                Command::Write(key.to_string(), rest[value_start..].to_string())
            } else {
                Command::Invalid(message.to_string())
            }
        }
        "READ" => {
            if words.len() == 2 {
                Command::Read(words[1].to_string())
            } else {
                Command::Invalid(message.to_string())
            }
        }
        "DELETE" => {
            if words.len() == 2 {
                Command::Delete(words[1].to_string())
            } else {
                Command::Invalid(message.to_string())
            }
        }
        "COMPACT" => Command::Compact,
        "STATS" => Command::Stats,
        _ => Command::Invalid(message.to_string()),
    }
}

fn maybe_trigger_compaction(
    database: Arc<RwLock<Box<dyn StorageEngine>>>,
    stats: &Arc<Stats>,
    compaction_ratio: f32,
    compaction_max_segment: usize,
) {
    let db_clone_read = Arc::clone(&database);
    let db_clone_write = Arc::clone(&database);
    let should_compact = {
        let db = db_clone_read.read().unwrap();
        (compaction_ratio > 0.0
            && db.total_bytes() > 0
            && db.dead_bytes() as f32 / db.total_bytes() as f32 > compaction_ratio)
            || (compaction_max_segment > 0 && db.segment_count() > compaction_max_segment)
    };

    let stats_clone = Arc::clone(stats);
    if should_compact {
        if stats
            .compacting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        stats
            .last_compact_start_ms
            .store(Stats::now_ms(), Ordering::Relaxed);
        thread::spawn(move || {
            let mut db = db_clone_write.write().unwrap();
            db.compact().unwrap();

            stats_clone
                .last_compact_end_ms
                .store(Stats::now_ms(), Ordering::Relaxed);
            stats_clone.compacting.store(false, Ordering::Release);
            stats_clone.compaction_count.fetch_add(1, Ordering::Relaxed);
        });
    }
}
