use std::io;

pub trait StorageEngine: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<(String, String)>, std::io::Error>;
    fn set(&mut self, key: &str, value: &str) -> Result<(), std::io::Error>;
    fn delete(&mut self, key: &str) -> Result<Option<()>, std::io::Error>;
    fn compact(&mut self) -> Result<(), std::io::Error>;
    fn dead_bytes(&self) -> u64;
    fn total_bytes(&self) -> u64;
    fn segment_count(&self) -> usize;
    fn list_keys(&self) -> io::Result<Vec<String>>;
    fn exists(&self, key: &str) -> bool;
}
