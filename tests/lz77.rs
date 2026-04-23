use rustikv::lz77::Lz77;

#[test]
fn test_lz77_roundtrip_simple() {
    let original = b"hello world";
    let compressed = Lz77::encode(original);
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_roundtrip_with_repetition() {
    let original = b"the quick brown fox jumps over the lazy dog";
    let compressed = Lz77::encode(original);
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_roundtrip_highly_repetitive() {
    let original = b"hello hello hello hello hello";
    let compressed = Lz77::encode(original);
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_roundtrip_large_data() {
    let original: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
    let compressed = Lz77::encode(&original);
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_empty() {
    let original = b"";
    let compressed = Lz77::encode(original);
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_single_byte() {
    let original = b"a";
    let compressed = Lz77::encode(original);
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_two_bytes() {
    let original = b"ab";
    let compressed = Lz77::encode(original);
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_no_repetition() {
    let original = b"abcdefghijklmnopqrstuvwxyz";
    let compressed = Lz77::encode(original);
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_pattern_at_boundary() {
    // Test with pattern that repeats after 3 bytes (minimum match length)
    let original = b"abcabcabcabc";
    let compressed = Lz77::encode(original);
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_long_repetition() {
    // Test with long repetition to exercise lookahead window
    let mut original = Vec::new();
    for _ in 0..100 {
        original.extend_from_slice(b"test");
    }
    let compressed = Lz77::encode(&original);
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_compression_ratio() {
    // Highly repetitive data should compress better
    let repetitive: Vec<u8> = (0..1000).map(|_| b'a').collect();
    let compressed = Lz77::encode(&repetitive);

    // Compression should reduce size significantly for repetitive data
    // (not requiring exact ratio, just that it's smaller than original)
    assert!(compressed.len() < repetitive.len());
}

#[test]
fn test_lz77_random_data() {
    // Random-ish data with some patterns
    let original = b"the quick brown fox jumps over the lazy dog the quick brown fox";
    let compressed = Lz77::encode(original);
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_binary_data() {
    // Test with non-text binary data
    let original: Vec<u8> = vec![0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00];
    let compressed = Lz77::encode(&original);
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, original);
}

#[test]
fn test_lz77_mixed_patterns() {
    // Mix of patterns and non-patterns
    let mut original = Vec::new();
    original.extend_from_slice(b"unique");
    original.extend_from_slice(b"aaaa");
    original.extend_from_slice(b"bbbb");
    original.extend_from_slice(b"aaaa");

    let compressed = Lz77::encode(&original);
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, original);
}
