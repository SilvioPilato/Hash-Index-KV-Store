use std::{
    env::{self},
    net::SocketAddr,
};

pub struct Settings {
    pub db_file_path: String,
    pub tcp_addr: String,
    pub db_name: String,
    pub max_segment_bytes: u64,
}

impl Settings {
    pub fn get_from_args() -> Settings {
        let f_path = env::args().nth(1).expect("No destination file given");
        let mut args = env::args().skip(2);
        let mut settings = Settings {
            db_file_path: f_path,
            tcp_addr: "0.0.0.0:6666".to_string(),
            db_name: "segment".to_string(),
            max_segment_bytes: 1_048_576 * 50,
        };
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-t" | "--tcp" => {
                    if let Some(value) = args.next() {
                        let addr: SocketAddr = value.parse().expect("Invalid tcp address provided");
                        settings.tcp_addr = addr.to_string();
                    }
                }
                "-n" | "--name" => {
                    if let Some(value) = args.next() {
                        let name: String = value.parse().expect("Invalid db name");
                        settings.db_name = name.to_string();
                    }
                }
                "-msb" | "--max-segments-bytes" => {
                    if let Some(value) = args.next() {
                        let bytes: u64 =
                            value.parse().expect("Invalid max segments bytes provided");
                        settings.max_segment_bytes = bytes;
                    }
                }
                _ => println!("Unknown argument: {}", arg),
            }
        }

        settings
    }
}
