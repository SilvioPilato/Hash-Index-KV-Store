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
        cmd => ParseResult::InvalidInput(format!("Unknown command: {cmd}")),
    }
}
