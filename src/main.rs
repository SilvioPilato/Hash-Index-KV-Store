use hash_index::bffp::{Command, ResponseStatus, decode_input_frame, encode_frame};
use hash_index::engine::StorageEngine;
use hash_index::kvengine::KVEngine;
use hash_index::lsmengine::LsmEngine;
use hash_index::record::{MAX_KEY_SIZE, MAX_VALUE_SIZE};
use hash_index::settings::{EngineType, Settings};
use hash_index::stats::Stats;
use std::env;
use std::io::{self};
use std::{
    io::{BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, RwLock, atomic::Ordering},
    thread,
    time::Instant,
};

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
                        .write_all(&response)
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
) -> Vec<u8> {
    handle_stream_inner(
        stream,
        database,
        stats,
        compaction_ratio,
        compaction_max_segment,
    )
    .unwrap_or_else(|e| encode_frame(ResponseStatus::Error, &[e.to_string()]))
}

fn handle_stream_inner(
    stream: &TcpStream,
    database: Arc<RwLock<Box<dyn StorageEngine>>>,
    stats: &Arc<Stats>,
    compaction_ratio: f32,
    compaction_max_segment: usize,
) -> io::Result<Vec<u8>> {
    const MAX_REQUEST_BYTES: usize = MAX_KEY_SIZE + MAX_VALUE_SIZE + 1024;
    let mut reader = BufReader::new(stream);
    let mut len_buf = [0u8; 4];
    if reader.read_exact(&mut len_buf).is_err() {
        return Ok(encode_frame(
            ResponseStatus::Error,
            &["Received empty request.".to_string()],
        ));
    }
    let frame_len = u32::from_be_bytes(len_buf) as usize;
    if frame_len > MAX_REQUEST_BYTES {
        return Ok(encode_frame(
            ResponseStatus::Error,
            &[format!("Frame too large: {} bytes", frame_len)],
        ));
    }
    let mut payload = vec![0u8; frame_len];
    reader.read_exact(&mut payload)?;
    let mut buf = Vec::with_capacity(4 + frame_len);
    buf.extend_from_slice(&len_buf);
    buf.extend_from_slice(&payload);

    let cmd = decode_input_frame(&buf)?;

    Ok(match cmd {
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
                    encode_frame(ResponseStatus::Ok, &[])
                }
                Err(err) => encode_frame(ResponseStatus::Error, &[err.to_string()]),
            }
        }
        Command::Read(key) => {
            log_verbose(format!("Parsed READ command: key='{}'", key));
            let db = database.read().unwrap();
            stats.reads.fetch_add(1, Ordering::Relaxed);
            match db.get(&key) {
                Ok(Some((_, v))) => encode_frame(ResponseStatus::Ok, &[v]),
                Ok(None) => encode_frame(ResponseStatus::NotFound, &[]),
                Err(error) => encode_frame(ResponseStatus::Error, &[error.to_string()]),
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
                    encode_frame(ResponseStatus::Ok, &[])
                }
                Ok(None) => encode_frame(ResponseStatus::NotFound, &[]),
                Err(error) => encode_frame(ResponseStatus::Error, &[error.to_string()]),
            }
        }
        Command::Compact => {
            log_verbose("Parsed COMPACT command");
            if stats
                .compacting
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Ok(encode_frame(ResponseStatus::Noop, &[]));
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
            encode_frame(ResponseStatus::Ok, &[])
        }
        Command::Stats => encode_frame(ResponseStatus::Ok, &[stats.snapshot()]),
        Command::Invalid(op_code) => {
            log_verbose(format!("Invalid op code: {}", op_code));
            encode_frame(
                ResponseStatus::Error,
                &[format!("Invalid op code: {}", op_code)],
            )
        }
        Command::List => {
            let db = database.read().unwrap();
            match db.list_keys() {
                Ok(keys) => encode_frame(ResponseStatus::Ok, &keys),
                Err(error) => encode_frame(ResponseStatus::Error, &[error.to_string()]),
            }
        }
        Command::Exists(key) => {
            let db = database.read().unwrap();
            if db.exists(&key) {
                encode_frame(ResponseStatus::Ok, &[])
            } else {
                encode_frame(ResponseStatus::NotFound, &[])
            }
        }
    })
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
