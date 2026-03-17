use std::collections::BTreeMap;

pub struct Memtable {
    entries: BTreeMap<String, Option<String>>,
    size_bytes: usize,
}

impl Default for Memtable {
    fn default() -> Self {
        Self::new()
    }
}

impl Memtable {
    pub fn new() -> Self {
        Memtable {
            entries: BTreeMap::new(),
            size_bytes: 0,
        }
    }

    pub fn entry(&self, key: &str) -> Option<&Option<String>> {
        self.entries.get(key)
    }

    pub fn insert(&mut self, key: String, value: String) {
        let added = key.len() + value.len();
        if let Some(old) = self.entries.insert(key.clone(), Some(value)) {
            // subtract old value size (key was already counted)
            self.size_bytes -= old.as_ref().map_or(0, |v| v.len());
        } else {
            self.size_bytes += key.len();
        }
        self.size_bytes += added - key.len();
    }
    pub fn remove(&mut self, key: String) {
        let key_len = key.len();
        match self.entries.insert(key, None) {
            Some(Some(old_value)) => {
                // Key existed with a value — subtract the value size
                self.size_bytes -= old_value.len();
            }
            Some(None) => {
                // Key already had a tombstone — nothing to change
            }
            None => {
                // Brand new key — count the key size
                self.size_bytes += key_len;
            }
        }
    }

    pub fn size_bytes(&self) -> usize {
        self.size_bytes
    }
    pub fn entries(&self) -> &BTreeMap<String, Option<String>> {
        &self.entries
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.size_bytes = 0;
    }

    // Add to Memtable
    pub fn drop_tombstones(&mut self) {
        self.entries.retain(|_, v| v.is_some());
        // Recalculate size_bytes since we removed keys
        self.size_bytes = self
            .entries
            .iter()
            .map(|(k, v)| k.len() + v.as_ref().map_or(0, |v| v.len()))
            .sum();
    }
}
