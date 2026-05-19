use std::collections::BTreeMap;

use crate::record::TTL_LEN;

pub struct Memtable {
    entries: BTreeMap<String, MemtableEntry>,
    size_bytes: usize,
}

pub struct MemtableEntry {
    pub value: Option<String>, // None = tombstone
    pub expiry_ms: Option<u64>,
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

    pub fn entry(&self, key: &str) -> Option<&MemtableEntry> {
        self.entries.get(key)
    }

    pub fn insert(&mut self, key: String, value: String, expiry_ms: Option<u64>) {
        let ttl_bytes = if expiry_ms.is_some() { TTL_LEN } else { 0 };
        let added = key.len() + value.len() + ttl_bytes;
        let entry = MemtableEntry {
            value: Some(value),
            expiry_ms,
        };
        if let Some(old) = self.entries.insert(key.clone(), entry) {
            // subtract old value size (key was already counted)
            self.size_bytes -= old.value.as_ref().map_or(0, |v| v.len());
            self.size_bytes -= old.expiry_ms.as_ref().map_or(0, |_| TTL_LEN);
        } else {
            self.size_bytes += key.len();
        }
        self.size_bytes += added - key.len();
    }
    pub fn remove(&mut self, key: String) {
        let key_len = key.len();
        let deleted_entry = MemtableEntry {
            value: None,
            expiry_ms: None,
        };
        match self.entries.insert(key, deleted_entry) {
            Some(old_entry) => {
                // Key existed with a value — subtract the value size
                self.size_bytes -= old_entry.value.as_ref().map_or(0, |v| v.len());
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
    pub fn entries(&self) -> &BTreeMap<String, MemtableEntry> {
        &self.entries
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.size_bytes = 0;
    }

    // Add to Memtable
    pub fn drop_tombstones(&mut self) {
        self.entries.retain(|_, v| v.value.is_some());
        // Recalculate size_bytes since we removed keys
        self.size_bytes = self
            .entries
            .iter()
            .map(|(k, m)| {
                k.len() + m.value.as_ref().map_or(0, |v| v.len()) + m.expiry_ms.map_or(0, |_| 8)
            })
            .sum();
    }
}
