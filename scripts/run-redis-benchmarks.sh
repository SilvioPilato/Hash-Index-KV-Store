#!/bin/bash
# Run complete redis-compare benchmarks with different payload sizes
# and generate analysis report.

set -e

echo "=========================================="
echo "Redis Comparison Benchmark Suite"
echo "=========================================="
echo ""
echo "Prerequisites:"
echo "  1. Redis running: docker run -d -p 6379:6379 redis"
echo "  2. rustikv running: cargo run --release -- /tmp/bench-db --engine lsm --fsync-interval never"
echo ""
echo "Press Enter to start, or Ctrl+C to cancel..."
read

BENCHMARK_DIR="docs/benchmark-runs/2026-04-25"
mkdir -p "$BENCHMARK_DIR"

# Array of (payload_size_bytes, key_count)
declare -a PAYLOADS=(
    "100 10000"
    "1000 10000"
    "10000 1000"
    "100000 100"
)

echo ""
echo "Running benchmarks..."
echo ""

for payload_spec in "${PAYLOADS[@]}"; do
    size=$(echo $payload_spec | awk '{print $1}')
    count=$(echo $payload_spec | awk '{print $2}')

    # Convert to human-readable format
    if [ $size -eq 100 ]; then
        label="100B"
    elif [ $size -eq 1000 ]; then
        label="1KB"
    elif [ $size -eq 10000 ]; then
        label="10KB"
    elif [ $size -eq 100000 ]; then
        label="100KB"
    else
        label="${size}B"
    fi

    echo "--- Running benchmark: $label (count=$count) ---"
    output_file="$BENCHMARK_DIR/redis-compare-${label}.txt"

    cargo run --release --bin redis-compare -- \
        --count $count \
        --value-size $size \
        2>&1 | tee "$output_file"

    echo ""
    echo "Saved to: $output_file"
    echo ""
done

echo ""
echo "=========================================="
echo "Generating analysis report..."
echo "=========================================="
echo ""

python3 scripts/analyze-redis-compare.py

echo ""
echo "=========================================="
echo "All benchmarks complete!"
echo "=========================================="
echo ""
echo "Results saved to:"
echo "  - $BENCHMARK_DIR/redis-compare-*.txt (raw results)"
echo "  - $BENCHMARK_DIR/redis-comparison-analysis.md (summary report)"
echo ""
echo "Next steps:"
echo "  1. Review redis-comparison-analysis.md"
echo "  2. Compare to in-process (engbench) results"
echo "  3. Calculate TCP overhead: engbench vs kvbench throughput"
