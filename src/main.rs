use std::{env, io::{BufRead, BufReader, Write}, net::{TcpListener, TcpStream}};
use db::DB;

mod db;
mod hash_index;
mod utils;

enum Command {
    Write(String, String),
    Read(String),
    Delete(String),
    Invalid(String),
}


fn main() {
    let f_path = env::args().nth(1).expect("No command given");
    let mut database = DB::new(&f_path);
    let addr = "0.0.0.0:6666";
    let listener = TcpListener::bind(addr).unwrap();

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                println!("New connection: {}", stream.peer_addr().unwrap());
                let response = handle_stream(&stream, &mut database);
                stream.write_all(response.as_bytes()).expect("Could not write response to stream");
            }
            Err(e) => {
                eprintln!("Error accepting connection: {}", e);
            }
        }
    }
}

fn handle_stream(stream: &TcpStream, database: &mut DB) -> String {
    let reader = BufReader::new(stream);
    let request: Vec<_> = reader.
    lines()
    .map(|result| result.unwrap_or_else(|_| String::new()))
    .take_while(|line|!line.is_empty())
    .collect();

    if request.is_empty() {
        return "Received empty request.".to_string();
    }

    match parse_message(request.concat()) {
        Command::Write(key, value) => {
            println!("Parsed WRITE command: key='{}', value='{}'", key, value);
            database.set(&key, &value);
            "OK".to_string()
        }
        Command::Read(key) => {
            println!("Parsed READ command: key='{}'", key);
            database.get(&key).unwrap_or("Key not found".to_string())
        }
        Command::Delete(key) => {
            println!("Parsed DELETE command: key='{}'", key);
            database.delete(&key);
            "OK".to_string()
        }
        Command::Invalid(original_command) => {
            println!("Invalid command: {}", original_command);
            format!("Invalid command: {}", original_command)
        }
    }
}

fn parse_message(message: String) -> Command {
    let words: Vec<&str>  = message.trim().split_whitespace().collect();

    if words.is_empty() {
        return Command::Invalid(message);
    }

   match words[0].to_uppercase().as_str() {
        "WRITE" => {
            Command::Write(words[1].to_string(), words[2..].concat())
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
        _ => Command::Invalid(message),
   }
}

// WRITE KEY VALUE
// READ VALUE
// DELETE VALUE