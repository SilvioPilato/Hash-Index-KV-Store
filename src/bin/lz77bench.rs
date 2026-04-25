use rustikv::lz77::Lz77;
use std::time::Instant;

fn bench(label: &str, data: &[u8]) {
    let start = Instant::now();
    let compressed = Lz77::encode(data);
    let elapsed = start.elapsed();

    let original = data.len();
    let compressed_size = compressed.len();
    let ratio = compressed_size as f64 / original as f64;
    let throughput = original as f64 / elapsed.as_secs_f64() / (1024.0 * 1024.0);

    println!(
        "{label}|{original}|{compressed_size}|{ratio:.4}|{elapsed_ms}|{throughput:.2}",
        elapsed_ms = elapsed.as_millis(),
    );

    // Verify roundtrip
    let decompressed = Lz77::decode(&compressed);
    assert_eq!(decompressed, data, "Roundtrip failed for: {label}");
}

fn main() {
    // Print CSV header
    println!("label|original_bytes|compressed_bytes|ratio|elapsed_ms|throughput_mb_s");

    // --- Compression ratio tests ---

    // 1. Uniform repetitive: all same byte
    let uniform = vec![b'a'; 10_000];
    bench("Uniform (10 KB, all 'a')", &uniform);

    // 2. Highly repetitive text
    let hello: Vec<u8> = "hello world ".repeat(500).into_bytes();
    bench("Repetitive text (6 KB, 'hello world' x500)", &hello);

    // 3. Mixed: unique prefix + repetitive middle + unique suffix
    let mut mixed = Vec::new();
    for i in 0u8..128 {
        mixed.push(i);
    }
    mixed.extend(b"pattern".repeat(1_000));
    for i in (0u8..128).rev() {
        mixed.push(i);
    }
    bench("Mixed (unique+repetitive+unique)", &mixed);

    // 4. Natural text: Lorem ipsum (approx 500 KB by repeating)
    let lorem = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
        Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. \
        Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris. \
        Nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in \
        reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla \
        pariatur. Excepteur sint occaecat cupidatat non proident, sunt in \
        culpa qui officia deserunt mollit anim id est laborum. ";
    let lorem_500kb: Vec<u8> = lorem.repeat(1_500).into_bytes(); // ~600 KB
    bench("Natural text (Lorem ipsum ~600 KB)", &lorem_500kb);

    // 5. Binary pattern: 0xFF/0x00 alternating
    let binary_pattern: Vec<u8> = (0..102_400)
        .map(|i| if i % 2 == 0 { 0xFF } else { 0x00 })
        .collect();
    bench("Binary alternating (100 KB, 0xFF/0x00)", &binary_pattern);

    // 6. Pseudo-random bytes (LCG)
    let mut random: Vec<u8> = Vec::with_capacity(102_400);
    let mut state: u64 = 0xDEAD_BEEF_CAFE_BABE;
    for _ in 0..102_400 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        random.push((state >> 56) as u8);
    }
    bench("Random (100 KB, LCG pseudo-random)", &random);

    // --- Performance tests ---

    // 7. 1 MB uniform bytes
    let uniform_1mb = vec![b'x'; 1_048_576];
    bench("Perf: 1 MB uniform bytes", &uniform_1mb);

    // 8. 10 MB repetitive text
    let rep_10mb: Vec<u8> = "the quick brown fox ".repeat(500_000).into_bytes();
    let rep_10mb_trimmed = &rep_10mb[..10_485_760.min(rep_10mb.len())];
    bench("Perf: 10 MB repetitive text", rep_10mb_trimmed);

    // 9. 1 MB random data
    let mut random_1mb: Vec<u8> = Vec::with_capacity(1_048_576);
    let mut state2: u64 = 0x1234_5678_9ABC_DEF0;
    for _ in 0..1_048_576 {
        state2 = state2
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        random_1mb.push((state2 >> 56) as u8);
    }
    bench("Perf: 1 MB random data", &random_1mb);
}
