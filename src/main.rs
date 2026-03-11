use db::DB;
use stats::Stats;
use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, RwLock, atomic::Ordering},
    thread,
    time::Instant,
};

use crate::settings::Settings;

mod db;
mod hash_index;
mod segment;
mod settings;
mod stats;
mod utils;

enum Command {
    Write(String, String),
    Read(String),
    Delete(String),
    Invalid(String),
    Compact,
    Stats,
}

fn main() {
    let settings = Settings::get_from_args();

    let database = match DB::from_dir(&settings.db_file_path, &settings.db_name).unwrap() {
        Some(db) => db,
        None => DB::new(&settings.db_file_path, &settings.db_name),
    };
    let db_handle = Arc::new(RwLock::new(database));
    let stats = Arc::new(Stats::new());
    let listener = TcpListener::bind(&settings.tcp_addr).unwrap();

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                println!("New connection: {}", stream.peer_addr().unwrap());
                let shared_db = Arc::clone(&db_handle);
                let shared_stats = Arc::clone(&stats);
                thread::spawn(move || {
                    shared_stats
                        .active_connections
                        .fetch_add(1, Ordering::Relaxed);
                    let response = handle_stream(&stream, shared_db, &shared_stats);
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
}

fn handle_stream(stream: &TcpStream, database: Arc<RwLock<DB>>, stats: &Arc<Stats>) -> String {
    let reader = BufReader::new(stream);
    let request: Vec<_> = reader
        .lines()
        .map(|result| result.unwrap_or_else(|_| String::new()))
        .take_while(|line| !line.is_empty())
        .collect();

    if request.is_empty() {
        return "Received empty request.".to_string();
    }

    match parse_message(request.concat()) {
        Command::Write(key, value) => {
            println!("Parsed WRITE command: key='{}', value='{}'", key, value);
            if stats.compacting.load(Ordering::Relaxed) {
                stats.write_blocked_attempts.fetch_add(1, Ordering::Relaxed);
            }
            let lock_start = Instant::now();
            let mut db = database.write().unwrap();
            let lock_elapsed = lock_start.elapsed().as_millis() as u64;
            stats
                .write_blocked_total_ms
                .fetch_add(lock_elapsed, Ordering::Relaxed);

            match db.set(&key, &value) {
                Ok(_) => {
                    stats.writes.fetch_add(1, Ordering::Relaxed);
                    "OK".to_string()
                }
                Err(err) => err.to_string(),
            }
        }
        Command::Read(key) => {
            println!("Parsed READ command: key='{}'", key);
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
            println!("Parsed DELETE command: key='{}'", key);
            let mut db = database.write().unwrap();
            match db.delete(&key) {
                Ok(result) => match result {
                    Some(()) => {
                        stats.deletes.fetch_add(1, Ordering::Relaxed);
                        "OK".to_string()
                    }
                    None => "Not found".to_string(),
                },
                Err(error) => error.to_string(),
            }
        }
        Command::Compact => {
            println!("Parsed COMPACT command");
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
                let compacted = {
                    let db = db_clone.read().unwrap();
                    db.get_compacted()
                };

                let mut db = db_clone.write().unwrap();
                *db = compacted.unwrap();

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
            println!("Invalid command: {}", original_command);
            format!("Invalid command: {}", original_command)
        }
    }
}

fn parse_message(message: String) -> Command {
    let words: Vec<&str> = message.trim().split_whitespace().collect();

    if words.is_empty() {
        return Command::Invalid(message);
    }

    match words[0].to_uppercase().as_str() {
        "WRITE" => {
            if words.len() > 2 {
                Command::Write(words[1].to_string(), words[2..].join(" "))
            } else {
                Command::Invalid(message)
            }
        }
        "READ" => {
            if words.len() == 2 {
                Command::Read(words[1].to_string())
            } else {
                Command::Invalid(message)
            }
        }
        "DELETE" => {
            if words.len() == 2 {
                Command::Delete(words[1].to_string())
            } else {
                Command::Invalid(message)
            }
        }
        "COMPACT" => Command::Compact,
        "STATS" => Command::Stats,
        _ => Command::Invalid(message),
    }
}
