use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Stats {
    pub compacting: AtomicBool,
    pub compaction_count: AtomicU64,
    pub last_compact_start_ms: AtomicU64,
    pub last_compact_end_ms: AtomicU64,
    pub write_blocked_attempts: AtomicU64,
    pub write_blocked_total_ms: AtomicU64,
    pub reads: AtomicU64,
    pub writes: AtomicU64,
    pub deletes: AtomicU64,
    pub active_connections: AtomicI64,
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

impl Stats {
    pub fn new() -> Stats {
        Stats {
            compacting: AtomicBool::new(false),
            compaction_count: AtomicU64::new(0),
            last_compact_start_ms: AtomicU64::new(0),
            last_compact_end_ms: AtomicU64::new(0),
            write_blocked_attempts: AtomicU64::new(0),
            write_blocked_total_ms: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            deletes: AtomicU64::new(0),
            active_connections: AtomicI64::new(0),
        }
    }

    pub fn snapshot(&self) -> String {
        format!(
            "compacting={}\n\
             compaction_count={}\n\
             last_compact_start_ms={}\n\
             last_compact_end_ms={}\n\
             write_blocked_attempts={}\n\
             write_blocked_total_ms={}\n\
             reads={}\n\
             writes={}\n\
             deletes={}\n\
             active_connections={}",
            self.compacting.load(Ordering::Relaxed),
            self.compaction_count.load(Ordering::Relaxed),
            self.last_compact_start_ms.load(Ordering::Relaxed),
            self.last_compact_end_ms.load(Ordering::Relaxed),
            self.write_blocked_attempts.load(Ordering::Relaxed),
            self.write_blocked_total_ms.load(Ordering::Relaxed),
            self.reads.load(Ordering::Relaxed),
            self.writes.load(Ordering::Relaxed),
            self.deletes.load(Ordering::Relaxed),
            self.active_connections.load(Ordering::Relaxed),
        )
    }

    pub fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }
}
