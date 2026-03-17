use hash_index::bloom::BloomFilter;

#[test]
fn inserted_key_is_found() {
    let mut bloom = BloomFilter::new(128, 3);
    bloom.insert("hello");
    assert!(bloom.might_contain("hello"));
}

#[test]
fn multiple_inserted_keys_are_found() {
    let mut bloom = BloomFilter::new(128, 3);
    bloom.insert("apple");
    bloom.insert("banana");
    bloom.insert("cherry");

    assert!(bloom.might_contain("apple"));
    assert!(bloom.might_contain("banana"));
    assert!(bloom.might_contain("cherry"));
}

#[test]
fn empty_filter_returns_false() {
    let bloom = BloomFilter::new(128, 3);
    assert!(!bloom.might_contain("anything"));
    assert!(!bloom.might_contain(""));
    assert!(!bloom.might_contain("hello"));
}

#[test]
fn missing_key_not_found() {
    let mut bloom = BloomFilter::new(128, 3);
    bloom.insert("present");
    // Not a guarantee (could be a false positive), but with a
    // 1024-bit filter and only 1 key this should be reliable.
    assert!(!bloom.might_contain("absent"));
}

#[test]
fn no_false_negatives() {
    let mut bloom = BloomFilter::new(1024, 5);
    let keys: Vec<String> = (0..500).map(|i| format!("key_{}", i)).collect();

    for key in &keys {
        bloom.insert(key);
    }

    // Every inserted key MUST be found — zero false negatives allowed.
    for key in &keys {
        assert!(bloom.might_contain(key), "false negative for '{}'", key);
    }
}

#[test]
fn false_positive_rate_is_bounded() {
    // Insert 100 keys into a filter sized to keep FP rate low.
    // 1024 bytes = 8192 bits, 100 keys, 7 hashes → theoretical FP ≈ 0.8%
    let mut bloom = BloomFilter::new(1024, 7);
    for i in 0..100 {
        bloom.insert(&format!("inserted_{}", i));
    }

    // Test 10_000 keys that were NOT inserted.
    let false_positives = (0..10_000)
        .filter(|i| bloom.might_contain(&format!("not_inserted_{}", i)))
        .count();

    let fp_rate = false_positives as f64 / 10_000.0;
    // Allow up to 5% — well above the theoretical ~0.8%, so this should
    // never flake while still catching a broken implementation.
    assert!(
        fp_rate < 0.05,
        "false positive rate too high: {:.2}% ({} / 10000)",
        fp_rate * 100.0,
        false_positives
    );
}

#[test]
fn different_hash_counts_all_work() {
    for k in 1..=10 {
        let mut bloom = BloomFilter::new(256, k);
        bloom.insert("test_key");
        assert!(
            bloom.might_contain("test_key"),
            "failed with hash_count = {}",
            k
        );
    }
}

#[test]
fn single_byte_filter_works() {
    // Smallest possible filter: 1 byte = 8 bits
    let mut bloom = BloomFilter::new(1, 2);
    bloom.insert("x");
    assert!(bloom.might_contain("x"));
}
