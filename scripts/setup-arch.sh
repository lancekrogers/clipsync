#!/bin/bash
set -e

# ClipSync setup script for Arch Linux / Endeavour OS

echo "Setting up ClipSync development environment for Arch Linux..."
echo "============================================================="

# Check if we're on Arch
if ! command -v pacman &> /dev/null; then
    echo "❌ This script is for Arch Linux systems only"
    exit 1
fi

echo "✅ Detected Arch Linux system"

# Install dependencies
echo ""
echo "📦 Installing dependencies..."
sudo pacman -S --needed \
    rust \
    cargo \
    gcc \
    pkg-config \
    openssl \
    libx11 \
    libxcb \
    libxrandr \
    dbus \
    git \
    base-devel

# Install additional development tools
echo ""
echo "🔧 Installing development tools..."
sudo pacman -S --needed \
    rust-analyzer \
    gdb \
    valgrind \
    time

# Install cross-compilation support
echo ""
echo "🎯 Setting up cross-compilation..."
rustup target add x86_64-unknown-linux-gnu
rustup target add aarch64-unknown-linux-gnu

# Install cargo tools
echo ""
echo "⚡ Installing cargo tools..."
cargo install --locked \
    cargo-watch \
    cargo-audit \
    cargo-tarpaulin \
    cargo-criterion \
    cross

# Create optimized cargo config for Arch
echo ""
echo "⚙️ Creating Arch-optimized cargo config..."
mkdir -p ~/.cargo

cat > ~/.cargo/config.toml << 'EOF'
# Arch Linux optimized cargo configuration

[build]
# Use all CPU cores
jobs = 0

# Arch-specific optimizations
[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = [
    "-C", "link-arg=-fuse-ld=lld",  # Use LLD linker (faster)
    "-C", "target-cpu=native",       # Optimize for your CPU
]

# Fast development builds
[profile.dev]
opt-level = 1
debug = true
incremental = true
codegen-units = 256  # Fast parallel compilation

# Fast CI builds
[profile.ci]
inherits = "release"
opt-level = 2
lto = "thin"
codegen-units = 4
strip = true
panic = "abort"

# Optimized release builds
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
strip = true
panic = "abort"

[env]
# Use system OpenSSL
OPENSSL_NO_VENDOR = "1"
EOF

echo ""
echo "🚀 Setting up fast development workflow..."

# Create a justfile for common tasks
cat > justfile << 'EOF'
# ClipSync development commands

# Fast development build
dev:
    cargo build --profile dev

# Watch and rebuild on changes  
watch:
    cargo watch -x "build --profile dev"

# Quick check without building
check:
    cargo check

# Run tests
test:
    cargo test

# Run with logging
run *args:
    RUST_LOG=debug cargo run -- {{args}}

# Benchmark
bench:
    cargo criterion

# Profile with perf (Arch-specific)
profile:
    cargo build --profile release
    perf record --call-graph=dwarf ./target/release/clipsync --help
    perf report

# Quick install locally
install:
    cargo install --path . --force

# Clean everything
clean:
    cargo clean
    rm -rf target
EOF

echo ""
echo "✅ Setup complete!"
echo ""
echo "🎉 Arch Linux optimizations applied:"
echo "   • LLD linker for faster linking"
echo "   • Native CPU optimization"  
echo "   • Parallel compilation (all cores)"
echo "   • System OpenSSL (no vendoring)"
echo ""
echo "⚡ Quick commands:"
echo "   just dev     - Fast development build (~5-15s)"
echo "   just watch   - Auto-rebuild on changes"
echo "   just check   - Type checking only (~2-5s)"
echo "   just run     - Run with debug logging"
echo ""
echo "🔥 Expected build times on Arch:"
echo "   • Development build: 5-15 seconds"
echo "   • Clean release build: 30-45 seconds"
echo "   • Incremental build: 1-3 seconds"