use std::time::{SystemTime, UNIX_EPOCH};

pub fn is_expired(expiry_ms: u64, now_ms: u64) -> bool {
    expiry_ms <= now_ms
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before UNIX epoch")
        .as_millis() as u64
}
