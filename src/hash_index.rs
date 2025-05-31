use std::{collections::HashMap};

pub struct HashIndex {
    hashmap: HashMap<String, u64>
}

impl HashIndex {
    pub(crate) fn new() -> HashIndex {
        let hashmap = HashMap::new();
        return HashIndex { hashmap }
    }

    pub fn get(&self, key: &str) -> Option<&u64>  {
        self.hashmap.get(key)
    }

    pub fn set(&mut self, key: String, location: u64)  {
        self.hashmap.insert(key, location);
    }

    pub fn ls_keys(&mut self) -> std::collections::hash_map::Keys<'_, String, u64> {
        self.hashmap.keys()
    }

    pub fn delete(&mut self, key: &str) -> Option<u64> {
        self.hashmap.remove(key)
    }
}