use rustikv::memtable::Memtable;

/// Extracts the stored value for `key`:
/// `None` = key absent, `Some(None)` = tombstone, `Some(Some(v))` = live value.
fn value_of(mt: &Memtable, key: &str) -> Option<Option<String>> {
    mt.entry(key).map(|e| e.value.clone())
}

#[test]
fn insert_and_entry() {
    let mut mt = Memtable::new();
    mt.insert("hello".to_string(), "world".to_string(), None);
    assert_eq!(value_of(&mt, "hello"), Some(Some("world".to_string())));
}

#[test]
fn entry_missing_key() {
    let mt = Memtable::new();
    assert_eq!(value_of(&mt, "nope"), None);
}

#[test]
fn insert_overwrite() {
    let mut mt = Memtable::new();
    mt.insert("k".to_string(), "old".to_string(), None);
    mt.insert("k".to_string(), "new".to_string(), None);
    assert_eq!(value_of(&mt, "k"), Some(Some("new".to_string())));
}

#[test]
fn remove_creates_tombstone() {
    let mut mt = Memtable::new();
    mt.insert("k".to_string(), "v".to_string(), None);
    mt.remove("k".to_string());
    // Tombstone: key exists but value is None
    assert_eq!(value_of(&mt, "k"), Some(None));
}

#[test]
fn remove_nonexistent_creates_tombstone() {
    let mut mt = Memtable::new();
    mt.remove("ghost".to_string());
    assert_eq!(value_of(&mt, "ghost"), Some(None));
}

#[test]
fn size_bytes_tracks_inserts() {
    let mut mt = Memtable::new();
    assert_eq!(mt.size_bytes(), 0);
    mt.insert("ab".to_string(), "cd".to_string(), None); // 2 + 2 = 4
    assert_eq!(mt.size_bytes(), 4);
    mt.insert("ef".to_string(), "gh".to_string(), None); // + 2 + 2 = 8
    assert_eq!(mt.size_bytes(), 8);
}

#[test]
fn size_bytes_overwrite_adjusts() {
    let mut mt = Memtable::new();
    mt.insert("k".to_string(), "short".to_string(), None); // 1 + 5 = 6
    assert_eq!(mt.size_bytes(), 6);
    mt.insert("k".to_string(), "longer_value".to_string(), None); // key already counted, value 5 -> 12
    assert_eq!(mt.size_bytes(), 1 + 12);
}

#[test]
fn size_bytes_expiry_adds_eight_bytes() {
    let mut mt = Memtable::new();
    mt.insert("k".to_string(), "v".to_string(), Some(123)); // 1 + 1 + 8 (TTL)
    assert_eq!(mt.size_bytes(), 10);
}

#[test]
fn size_bytes_remove_subtracts_value() {
    let mut mt = Memtable::new();
    mt.insert("k".to_string(), "val".to_string(), None); // 1 + 3 = 4
    assert_eq!(mt.size_bytes(), 4);
    mt.remove("k".to_string()); // value removed, key stays: 1
    assert_eq!(mt.size_bytes(), 1);
}

#[test]
fn size_bytes_remove_new_key_adds_key_len() {
    let mut mt = Memtable::new();
    mt.remove("abc".to_string()); // brand new tombstone: key_len = 3
    assert_eq!(mt.size_bytes(), 3);
}

#[test]
fn size_bytes_remove_tombstone_is_noop() {
    let mut mt = Memtable::new();
    mt.remove("k".to_string()); // key_len = 1
    assert_eq!(mt.size_bytes(), 1);
    mt.remove("k".to_string()); // already a tombstone, no change
    assert_eq!(mt.size_bytes(), 1);
}

#[test]
fn clear_resets_everything() {
    let mut mt = Memtable::new();
    mt.insert("k1".to_string(), "v1".to_string(), None);
    mt.insert("k2".to_string(), "v2".to_string(), None);
    mt.clear();
    assert_eq!(mt.size_bytes(), 0);
    assert_eq!(value_of(&mt, "k1"), None);
    assert_eq!(value_of(&mt, "k2"), None);
}

#[test]
fn entries_returns_sorted_keys() {
    let mut mt = Memtable::new();
    mt.insert("cherry".to_string(), "3".to_string(), None);
    mt.insert("apple".to_string(), "1".to_string(), None);
    mt.insert("banana".to_string(), "2".to_string(), None);
    let keys: Vec<&String> = mt.entries().keys().collect();
    assert_eq!(keys, vec!["apple", "banana", "cherry"]);
}

#[test]
fn drop_tombstones_removes_nones() {
    let mut mt = Memtable::new();
    mt.insert("keep".to_string(), "yes".to_string(), None);
    mt.insert("gone".to_string(), "no".to_string(), None);
    mt.remove("gone".to_string());
    mt.drop_tombstones();
    assert_eq!(value_of(&mt, "keep"), Some(Some("yes".to_string())));
    assert_eq!(value_of(&mt, "gone"), None);
    // size_bytes should only reflect "keep" + "yes"
    assert_eq!(mt.size_bytes(), 4 + 3);
}
