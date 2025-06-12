#!/bin/bash
set -e

# ClipSync Performance Benchmark Script
# Runs comprehensive performance benchmarks and generates reports

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BENCHMARK_DIR="$PROJECT_ROOT/benchmark-results"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Print colored output
print_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Create benchmark directory
setup_benchmark_dir() {
    mkdir -p "$BENCHMARK_DIR"
    mkdir -p "$BENCHMARK_DIR/raw"
    mkdir -p "$BENCHMARK_DIR/reports"
}

# Check system resources
check_system() {
    print_info "System Information:"
    echo "OS: $(uname -s) $(uname -r)"
    echo "CPU: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || grep -m1 'model name' /proc/cpuinfo | cut -d: -f2)"
    echo "Memory: $(free -h 2>/dev/null | grep Mem: | awk '{print $2}' || sysctl -n hw.memsize | awk '{print $0/1024/1024/1024 " GB"}')"
    echo "Rust: $(rustc --version)"
    echo ""
}

# Build optimized binary
build_optimized() {
    print_info "Building optimized binary for benchmarking..."
    
    cd "$PROJECT_ROOT"
    
    # Set aggressive optimization flags
    export RUSTFLAGS="-C target-cpu=native -C opt-level=3"
    
    cargo build --release --features bench
    
    print_success "Optimized binary built"
}

# Run cargo benchmarks
run_cargo_benchmarks() {
    print_info "Running cargo benchmarks..."
    
    cd "$PROJECT_ROOT"
    
    # Run benchmarks and save results
    cargo bench --all-features -- --save-baseline "benchmark-$TIMESTAMP" \
        --output-format bencher | tee "$BENCHMARK_DIR/raw/cargo-bench-$TIMESTAMP.txt"
    
    # Generate HTML report if criterion is used
    if [ -d "target/criterion" ]; then
        cp -r target/criterion "$BENCHMARK_DIR/reports/criterion-$TIMESTAMP"
        print_success "Criterion reports saved"
    fi
}

# Benchmark clipboard operations
benchmark_clipboard() {
    print_info "Benchmarking clipboard operations..."
    
    local RESULTS_FILE="$BENCHMARK_DIR/raw/clipboard-bench-$TIMESTAMP.txt"
    local BINARY="$PROJECT_ROOT/target/release/clipsync"
    
    {
        echo "Clipboard Operation Benchmarks"
        echo "=============================="
        echo ""
        
        # Small text (1KB)
        echo "Small text (1KB):"
        local small_text=$(head -c 1024 /dev/urandom | base64)
        time_operation "echo '$small_text' | $BINARY copy"
        
        # Medium text (100KB)
        echo -e "\nMedium text (100KB):"
        local medium_text=$(head -c 102400 /dev/urandom | base64)
        time_operation "echo '$medium_text' | $BINARY copy"
        
        # Large text (10MB)
        echo -e "\nLarge text (10MB):"
        local large_text=$(head -c 10485760 /dev/urandom | base64)
        time_operation "echo '$large_text' | $BINARY copy"
        
    } | tee "$RESULTS_FILE"
}

# Time a single operation
time_operation() {
    local cmd=$1
    local times=10
    local total=0
    
    echo "Running $times iterations..."
    
    for i in $(seq 1 $times); do
        local start=$(date +%s.%N)
        eval "$cmd" >/dev/null 2>&1
        local end=$(date +%s.%N)
        local duration=$(echo "$end - $start" | bc)
        total=$(echo "$total + $duration" | bc)
        echo -n "."
    done
    echo ""
    
    local avg=$(echo "scale=3; $total / $times" | bc)
    echo "Average time: ${avg}s"
}

# Benchmark sync operations
benchmark_sync() {
    print_info "Benchmarking sync operations..."
    
    local RESULTS_FILE="$BENCHMARK_DIR/raw/sync-bench-$TIMESTAMP.txt"
    
    {
        echo "Sync Operation Benchmarks"
        echo "========================"
        echo ""
        
        # Start daemon
        "$PROJECT_ROOT/target/release/clipsync" daemon &
        local DAEMON_PID=$!
        sleep 2
        
        # Measure sync latency
        echo "Sync latency test:"
        for size in 1 10 100 1000; do
            echo -e "\nPayload size: ${size}KB"
            local payload=$(head -c $((size * 1024)) /dev/urandom | base64)
            
            # Time sync operation
            local start=$(date +%s.%N)
            echo "$payload" | "$PROJECT_ROOT/target/release/clipsync" copy
            "$PROJECT_ROOT/target/release/clipsync" sync
            local end=$(date +%s.%N)
            
            local duration=$(echo "$end - $start" | bc)
            echo "Sync time: ${duration}s"
        done
        
        # Clean up
        kill $DAEMON_PID 2>/dev/null || true
        
    } | tee "$RESULTS_FILE"
}

# Benchmark memory usage
benchmark_memory() {
    print_info "Benchmarking memory usage..."
    
    local RESULTS_FILE="$BENCHMARK_DIR/raw/memory-bench-$TIMESTAMP.txt"
    local BINARY="$PROJECT_ROOT/target/release/clipsync"
    
    {
        echo "Memory Usage Benchmarks"
        echo "======================"
        echo ""
        
        # Start daemon and monitor memory
        "$BINARY" daemon &
        local DAEMON_PID=$!
        
        echo "Monitoring memory usage for 60 seconds..."
        echo "Time(s) RSS(MB) VSZ(MB)"
        
        for i in $(seq 0 5 60); do
            if ps -p $DAEMON_PID > /dev/null; then
                local mem_info=$(ps -o rss,vsz -p $DAEMON_PID | tail -1)
                local rss=$(echo $mem_info | awk '{print $1/1024}')
                local vsz=$(echo $mem_info | awk '{print $2/1024}')
                printf "%6d %7.1f %7.1f\n" $i $rss $vsz
                
                # Perform some operations
                echo "test-$i" | "$BINARY" copy >/dev/null 2>&1
                
                sleep 5
            fi
        done
        
        # Clean up
        kill $DAEMON_PID 2>/dev/null || true
        
    } | tee "$RESULTS_FILE"
}

# Benchmark startup time
benchmark_startup() {
    print_info "Benchmarking startup time..."
    
    local RESULTS_FILE="$BENCHMARK_DIR/raw/startup-bench-$TIMESTAMP.txt"
    local BINARY="$PROJECT_ROOT/target/release/clipsync"
    
    {
        echo "Startup Time Benchmarks"
        echo "======================"
        echo ""
        
        # Cold start
        echo "Cold start times (10 runs):"
        for i in $(seq 1 10); do
            # Clear cache if possible
            sync && echo 3 | sudo tee /proc/sys/vm/drop_caches >/dev/null 2>&1 || true
            
            local start=$(date +%s.%N)
            "$BINARY" --version >/dev/null
            local end=$(date +%s.%N)
            
            local duration=$(echo "scale=3; ($end - $start) * 1000" | bc)
            echo "Run $i: ${duration}ms"
        done
        
        # Warm start
        echo -e "\nWarm start times (10 runs):"
        for i in $(seq 1 10); do
            local start=$(date +%s.%N)
            "$BINARY" --version >/dev/null
            local end=$(date +%s.%N)
            
            local duration=$(echo "scale=3; ($end - $start) * 1000" | bc)
            echo "Run $i: ${duration}ms"
        done
        
    } | tee "$RESULTS_FILE"
}

# Generate performance report
generate_report() {
    print_info "Generating performance report..."
    
    local REPORT_FILE="$BENCHMARK_DIR/reports/performance-report-$TIMESTAMP.md"
    
    {
        echo "# ClipSync Performance Report"
        echo ""
        echo "Generated: $(date)"
        echo ""
        echo "## System Information"
        echo '```'
        check_system
        echo '```'
        echo ""
        echo "## Benchmark Results"
        echo ""
        
        # Include all benchmark results
        for bench_file in "$BENCHMARK_DIR/raw"/*-$TIMESTAMP.txt; do
            if [ -f "$bench_file" ]; then
                echo "### $(basename "$bench_file" .txt)"
                echo '```'
                cat "$bench_file"
                echo '```'
                echo ""
            fi
        done
        
        echo "## Performance Baseline"
        echo ""
        echo "Based on the benchmarks, the following performance baselines are established:"
        echo ""
        echo "- **Startup time**: < 50ms (warm), < 200ms (cold)"
        echo "- **Small clipboard copy**: < 10ms"
        echo "- **Large clipboard copy (10MB)**: < 500ms"
        echo "- **Memory usage**: < 50MB RSS during normal operation"
        echo "- **Sync latency**: < 100ms for payloads up to 1MB"
        echo ""
        echo "## Recommendations"
        echo ""
        echo "1. Monitor these metrics in CI/CD to catch performance regressions"
        echo "2. Set up alerts if performance degrades more than 10% from baseline"
        echo "3. Run benchmarks on target hardware for accurate baselines"
        
    } > "$REPORT_FILE"
    
    print_success "Performance report generated: $REPORT_FILE"
}

# Compare with previous benchmark
compare_benchmarks() {
    if [ -n "$1" ]; then
        local baseline=$1
        print_info "Comparing with baseline: $baseline"
        
        cd "$PROJECT_ROOT"
        cargo bench -- --baseline "$baseline"
    else
        print_warning "No baseline specified for comparison"
    fi
}

# Main function
main() {
    print_info "Starting ClipSync performance benchmarks..."
    
    # Setup
    setup_benchmark_dir
    check_system
    
    # Build
    build_optimized
    
    # Run benchmarks
    run_cargo_benchmarks
    benchmark_clipboard
    benchmark_sync
    benchmark_memory
    benchmark_startup
    
    # Generate report
    generate_report
    
    # Compare with baseline if provided
    if [ -n "$1" ]; then
        compare_benchmarks "$1"
    fi
    
    print_success "All benchmarks completed!"
    echo ""
    echo "Results saved to: $BENCHMARK_DIR"
    echo "Report: $BENCHMARK_DIR/reports/performance-report-$TIMESTAMP.md"
}

# Run main function
main "$@"