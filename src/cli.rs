use crate::bffp::Command;

pub enum ParseResult {
    Cmd(Command),
    Quit,
    InvalidInput(String),
}

pub fn parse_command(line: &str) -> ParseResult {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() {
        return ParseResult::InvalidInput(String::new());
    }

    match words[0].to_uppercase().as_str() {
        "WRITE" => {
            if words.len() > 2 {
                ParseResult::Cmd(Command::Write(words[1].to_string(), words[2..].join(" ")))
            } else {
                ParseResult::InvalidInput("Usage: WRITE <key> <value>".to_string())
            }
        }
        "READ" => {
            if words.len() == 2 {
                ParseResult::Cmd(Command::Read(words[1].to_string()))
            } else {
                ParseResult::InvalidInput("Usage: READ <key>".to_string())
            }
        }
        "DELETE" => {
            if words.len() == 2 {
                ParseResult::Cmd(Command::Delete(words[1].to_string()))
            } else {
                ParseResult::InvalidInput("Usage: DELETE <key>".to_string())
            }
        }
        "EXISTS" => {
            if words.len() == 2 {
                ParseResult::Cmd(Command::Exists(words[1].to_string()))
            } else {
                ParseResult::InvalidInput("Usage: EXISTS <key>".to_string())
            }
        }
        "COMPACT" => ParseResult::Cmd(Command::Compact),
        "STATS" => ParseResult::Cmd(Command::Stats),
        "LIST" => ParseResult::Cmd(Command::List),
        "PING" => ParseResult::Cmd(Command::Ping),
        "QUIT" => ParseResult::Quit,
        "MGET" => {
            if words.len() < 2 {
                ParseResult::InvalidInput("Usage: MGET <key1> <key2> <keyn>".to_string())
            } else {
                let keys: Vec<String> = words[1..].iter().map(|k| k.to_string()).collect();
                ParseResult::Cmd(Command::Mget(keys))
            }
        }
        "MSET" => {
            if words.len() < 3 {
                ParseResult::InvalidInput("Usage: MSET <key1> <value1> <keyn> <valuen>".to_string())
            } else {
                let pairs: Vec<(String, String)> = words[1..]
                    .chunks_exact(2)
                    .filter_map(|chunk| {
                        if let [k, v] = chunk {
                            Some((k.to_string(), v.to_string()))
                        } else {
                            None
                        }
                    })
                    .collect();
                ParseResult::Cmd(Command::Mset(pairs))
            }
        }
        "RANGE" => {
            if words.len() == 3 {
                ParseResult::Cmd(Command::Range(words[1].to_string(), words[2].to_string()))
            } else {
                ParseResult::InvalidInput("Usage: RANGE <start> <end>".to_string())
            }
        }
        cmd => ParseResult::InvalidInput(format!("Unknown command: {cmd}")),
    }
}
