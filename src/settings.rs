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

#[derive(Copy, Clone)]
pub enum EngineType {
    KV,
    Lsm,
}

pub struct Settings {
    pub db_file_path: String,
    pub tcp_addr: String,
    pub db_name: String,
    pub max_segment_bytes: u64,
    pub sync_strategy: FSyncStrategy,
    pub engine: EngineType,
    pub compaction_ratio: f32,
    pub compaction_max_segment: usize,
}

impl Settings {
    pub fn get_from_args() -> Settings {
        let args: Vec<String> = env::args().collect();

        if args.len() < 2 || args.iter().any(|a| a == "-h" || a == "--help") {
            Self::print_help(&args[0]);
            std::process::exit(0);
        }

        let f_path = args[1].clone();
        let mut args_iter = args.iter().skip(2);
        let mut settings = Settings {
            db_file_path: f_path,
            tcp_addr: "0.0.0.0:6666".to_string(),
            db_name: "segment".to_string(),
            max_segment_bytes: 1_048_576 * 50,
            sync_strategy: FSyncStrategy::Always,
            engine: EngineType::KV,
            compaction_ratio: 0.0,
            compaction_max_segment: 0,
        };
        while let Some(arg) = args_iter.next() {
            match arg.as_str() {
                "-t" | "--tcp" => {
                    if let Some(value) = args_iter.next() {
                        let addr: SocketAddr = value.parse().expect("Invalid tcp address provided");
                        settings.tcp_addr = addr.to_string();
                    }
                }
                "-n" | "--name" => {
                    if let Some(value) = args_iter.next() {
                        settings.db_name = value.to_string();
                    }
                }
                "-msb" | "--max-segments-bytes" => {
                    if let Some(value) = args_iter.next() {
                        let bytes: u64 =
                            value.parse().expect("Invalid max segments bytes provided");
                        settings.max_segment_bytes = bytes;
                    }
                }
                "-fsync" | "--fsync-interval" => {
                    if let Some(value) = args_iter.next() {
                        settings.sync_strategy = Settings::parse_fsync(value).unwrap();
                    }
                }
                "-e" | "--engine" => {
                    if let Some(value) = args_iter.next() {
                        settings.engine = Settings::parse_engine(value).unwrap();
                    }
                }
                "-cr" | "--compaction-ratio" => {
                    if let Some(value) = args_iter.next() {
                        settings.compaction_ratio =
                            value.parse().expect("Invalid compaction ratio provided");
                    }
                }
                "-cms" | "--compaction-max-segments" => {
                    if let Some(value) = args_iter.next() {
                        settings.compaction_max_segment = value
                            .parse()
                            .expect("Invalid compaction max segments provided");
                    }
                }
                _ => println!("Unknown argument: {}", arg),
            }
        }

        settings
    }

    fn print_help(prog_name: &str) {
        println!("Usage: {} <db_path> [OPTIONS]", prog_name);
        println!();
        println!("ARGUMENTS:");
        println!("  <db_path>              Path to the database directory");
        println!();
        println!("OPTIONS:");
        println!("  -t, --tcp <ADDR>       TCP bind address (default: 0.0.0.0:6666)");
        println!("  -n, --name <NAME>      Database name prefix (default: segment)");
        println!("  -msb, --max-segments-bytes <BYTES>");
        println!("                         Max bytes per segment (default: 52428800)");
        println!("  -fsync, --fsync-interval <POLICY>");
        println!(
            "                         Fsync strategy: 'always', 'never', 'every:N', 'every:Ns'"
        );
        println!("                         (default: always)");
        println!("  -e, --engine <ENGINE>  Storage engine: 'kv' or 'lsm' (default: kv)");
        println!("  -cr, --compaction-ratio <RATIO>");
        println!(
            "                         Auto-compact when dead/total bytes exceeds ratio (default: 0.0 = disabled)"
        );
        println!("  -h, --help             Print this help message");
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

    fn parse_engine(s: &str) -> Result<EngineType, String> {
        match s {
            "kv" => Ok(EngineType::KV),
            "lsm" => Ok(EngineType::Lsm),
            _ => Err(format!("Unsupported engine provided: {s}")),
        }
    }
}
