pub trait StorageEngine: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<(String, String)>, std::io::Error>;
    fn set(&mut self, key: &str, value: &str) -> Result<(), std::io::Error>;
    fn delete(&mut self, key: &str) -> Result<Option<()>, std::io::Error>;
    fn compact(&mut self) -> Result<(), std::io::Error>;
}
