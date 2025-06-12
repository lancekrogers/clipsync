#!/bin/bash
set -e

# Quick build test to estimate actual build times

echo "Testing ClipSync build performance..."
echo "====================================="

# Clean build
echo "1. Clean build test..."
cargo clean
time cargo build --profile ci

echo ""
echo "2. Incremental build test..."
touch src/main.rs
time cargo build --profile ci

echo ""
echo "3. Check build test..."
time cargo check

echo ""
echo "4. Release build test..."
time cargo build --release

echo ""
echo "5. Binary size comparison:"
echo "CI profile:      $(ls -lh target/ci/clipsync 2>/dev/null | awk '{print $5}' || echo 'N/A')"
echo "Release profile: $(ls -lh target/release/clipsync 2>/dev/null | awk '{print $5}' || echo 'N/A')"

echo ""
echo "Build performance summary:"
echo "- Clean build (CI profile): Should be ~30-60 seconds"
echo "- Incremental build: Should be ~2-5 seconds"
echo "- Cargo check: Should be ~5-10 seconds"
echo ""
echo "For comparison:"
echo "- Go: ~5-15 seconds clean build"
echo "- Python: Instant (interpreted)"
echo "- Rust: ~30-90 seconds clean build (depending on dependencies)"