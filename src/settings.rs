use std::{
    env::{self},
    net::SocketAddr,
    time::Duration,
};

#[derive(Copy, Clone)]
pub enum FSyncStrategy {
    Always,
    EveryN(usize),
    Periodic(Duration),
    Never,
}

pub struct Settings {
    pub db_file_path: String,
    pub tcp_addr: String,
    pub db_name: String,
    pub max_segment_bytes: u64,
    pub sync_strategy: FSyncStrategy,
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
            sync_strategy: FSyncStrategy::Always,
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
                        settings.db_name = value.to_string();
                    }
                }
                "-msb" | "--max-segments-bytes" => {
                    if let Some(value) = args.next() {
                        let bytes: u64 =
                            value.parse().expect("Invalid max segments bytes provided");
                        settings.max_segment_bytes = bytes;
                    }
                }
                "-fsync" | "--fsync-interval" => {
                    if let Some(value) = args.next() {
                        settings.sync_strategy = Settings::parse_fsync(&value).unwrap();
                    }
                }
                _ => println!("Unknown argument: {}", arg),
            }
        }

        settings
    }
    fn parse_fsync(s: &str) -> Result<FSyncStrategy, String> {
        match s {
            "always" => Ok(FSyncStrategy::Always),
            "never" => Ok(FSyncStrategy::Never),
            _ => {
                let val = s
                    .strip_prefix("every:")
                    .ok_or_else(|| format!("unknown fsync policy: {s}"))?;

                if let Some(secs_str) = val.strip_suffix('s') {
                    let n: u64 = secs_str
                        .parse()
                        .map_err(|_| format!("invalid fsync interval: {val}"))?;
                    if n == 0 {
                        return Err("fsync interval must be > 0".into());
                    }
                    Ok(FSyncStrategy::Periodic(Duration::from_secs(n)))
                } else {
                    let n: usize = val
                        .parse()
                        .map_err(|_| format!("invalid fsync interval: {val}"))?;
                    if n == 0 {
                        return Err("fsync interval must be > 0".into());
                    }
                    Ok(FSyncStrategy::EveryN(n))
                }
            }
        }
    }
}
