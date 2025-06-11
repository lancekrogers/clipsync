# ClipSync build recipes

default:
  @just --list

# Build debug binary
build:
  cargo build

# Build release binary
release:
  cargo build --release

# Run all tests
test:
  cargo test --all-features
  cargo test --doc

# Run integration tests
test-integration:
  cargo test --test '*' -- --test-threads=1

# Run benchmarks
bench:
  cargo bench

# Format code
fmt:
  cargo fmt
  cargo clippy --fix --allow-staged

# Check formatting and lints
check:
  cargo fmt -- --check
  cargo clippy -- -D warnings

# Security audit
audit:
  cargo audit
  cargo deny check

# Clean build artifacts
clean:
  cargo clean
  rm -rf dist/

# Generate documentation
docs:
  cargo doc --no-deps --open

# Watch for changes and rebuild
watch:
  cargo watch -x build -x test

# Build for all platforms
build-all:
  cargo build --target x86_64-apple-darwin
  cargo build --target aarch64-apple-darwin
  cargo build --target x86_64-unknown-linux-gnu
  cargo build --target aarch64-unknown-linux-gnu

# Create release packages
package: release
  #!/usr/bin/env bash
  set -euo pipefail
  
  mkdir -p dist
  
  # macOS
  if [[ "$OSTYPE" == "darwin"* ]]; then
    cp target/release/clipsync dist/clipsync-macos
    tar -czf dist/clipsync-macos-$(uname -m).tar.gz -C dist clipsync-macos
  fi
  
  # Linux
  if [[ "$OSTYPE" == "linux-gnu"* ]]; then
    cp target/release/clipsync dist/clipsync-linux
    tar -czf dist/clipsync-linux-$(uname -m).tar.gz -C dist clipsync-linux
  fi

# Install locally
install: release
  cargo install --path .

# Setup development environment
dev-setup:
  rustup component add rustfmt clippy
  cargo install cargo-watch cargo-audit cargo-deny
  pre-commit install