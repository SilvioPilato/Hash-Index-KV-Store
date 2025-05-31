use std::{format, time::SystemTime};

pub fn get_new_db_file_name(db_file_path: &str) -> Result<String, std::time::SystemTimeError>  {
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(n) => Ok(format!("{}_{}.db", db_file_path, n.as_secs())),
        Err(_) => panic!("SystemTime before UNIX EPOCH!"),
    }
}