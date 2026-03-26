use std::{
    env,
    io::{self, BufRead, Read, Write, stdout},
    net::{SocketAddr, TcpStream},
};

use rustikv::bffp::{ResponseStatus, decode_response_frame, encode_command};
use rustikv::cli::{ParseResult, parse_command};

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let mut args_iter = args.iter().skip(1);
    let mut host = String::from("127.0.0.1:6666");

    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "-h" | "--host" => {
                if let Some(value) = args_iter.next() {
                    let addr: SocketAddr = value.parse().expect("Invalid tcp address provided");
                    host = addr.to_string();
                }
            }
            _ => println!("Unknown argument: {}", arg),
        }
    }

    let mut stream = TcpStream::connect(host)?;

    let stdin = io::stdin();

    loop {
        print!("rustikli> ");
        stdout().flush().unwrap();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break, // EOF (e.g. Ctrl+D)
            Ok(_) => {
                let line = line.trim();
                let cmd = match parse_command(line) {
                    ParseResult::Cmd(command) => encode_command(command),
                    ParseResult::Quit => {
                        println!("Cya!");
                        break;
                    }
                    ParseResult::InvalidInput(msg) => {
                        if !msg.is_empty() {
                            eprintln!("{msg}");
                        }
                        continue;
                    }
                };

                stream.write_all(&cmd)?;
                let mut len_buf = [0u8; 4];
                stream.read_exact(&mut len_buf)?;
                let frame_len = u32::from_be_bytes(len_buf) as usize;

                let mut payload = vec![0u8; frame_len];
                stream.read_exact(&mut payload)?;
                let mut full = Vec::with_capacity(4 + frame_len);
                full.extend_from_slice(&len_buf);
                full.extend_from_slice(&payload);

                match decode_response_frame(&full) {
                    Ok(res) => {
                        match res.status {
                            ResponseStatus::Ok => {
                                for row in res.payload {
                                    println!("{}", row);
                                }
                            }
                            ResponseStatus::Error => {
                                println!("Error >");
                                for row in res.payload {
                                    println!("{}", row);
                                }
                            }
                            ResponseStatus::NotFound => {
                                println!("Not found");
                            }
                            ResponseStatus::Noop => {
                                println!("NOOP");
                            }
                        }

                        stdout().flush().unwrap();
                    }
                    Err(e) => {
                        eprintln!("Error reading server response: {e}");
                    }
                }
            }
            Err(e) => {
                eprintln!("Error reading input: {e}");
                break;
            }
        }
    }

    Ok(())
}
