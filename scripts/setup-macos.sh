#!/bin/bash
set -e

# ClipSync setup script for macOS

echo "Setting up ClipSync development environment for macOS..."
echo "======================================================"

# Check if we're on macOS
if [[ "$OSTYPE" != "darwin"* ]]; then
    echo "❌ This script is for macOS only"
    exit 1
fi

echo "✅ Detected macOS $(sw_vers -productVersion)"

# Check for Homebrew
if ! command -v brew &> /dev/null; then
    echo "📦 Installing Homebrew..."
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
else
    echo "✅ Homebrew already installed"
fi

# Install dependencies
echo ""
echo "📦 Installing dependencies..."
brew install \
    rust \
    pkg-config \
    openssl \
    just \
    llvm

# Install additional development tools
echo ""
echo "🔧 Installing development tools..."
brew install \
    rust-analyzer \
    gdb \
    valgrind \
    gnu-time

# Install cross-compilation support
echo ""
echo "🎯 Setting up cross-compilation..."
rustup target add x86_64-apple-darwin
rustup target add aarch64-apple-darwin
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

# Create optimized cargo config for macOS
echo ""
echo "⚙️ Creating macOS-optimized cargo config..."
mkdir -p ~/.cargo

# Detect if we're on Apple Silicon or Intel
ARCH=$(uname -m)
if [[ "$ARCH" == "arm64" ]]; then
    NATIVE_TARGET="aarch64-apple-darwin"
    echo "🍎 Detected Apple Silicon (M1/M2/M3)"
else
    NATIVE_TARGET="x86_64-apple-darwin"
    echo "🍎 Detected Intel Mac"
fi

cat > ~/.cargo/config.toml << EOF
# macOS optimized cargo configuration

[build]
# Use all CPU cores
jobs = 0

# macOS-specific optimizations
[target.${NATIVE_TARGET}]
rustflags = [
    "-C", "target-cpu=native",       # Optimize for your CPU
    "-C", "link-arg=-Wl,-ld_classic", # Use classic linker on newer macOS
]

# Intel Mac optimization
[target.x86_64-apple-darwin]
rustflags = [
    "-C", "target-cpu=native",
]

# Apple Silicon optimization  
[target.aarch64-apple-darwin]
rustflags = [
    "-C", "target-cpu=native",
]

# Linux cross-compilation
[target.x86_64-unknown-linux-gnu]
linker = "x86_64-linux-gnu-gcc"

[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"

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
# Use Homebrew OpenSSL
OPENSSL_DIR = "$(brew --prefix openssl)"
OPENSSL_NO_VENDOR = "1"
# PKG_CONFIG_PATH for Homebrew
PKG_CONFIG_PATH = "$(brew --prefix)/lib/pkgconfig"
EOF

echo ""
echo "🚀 Setting up fast development workflow..."

# Create a justfile for common tasks
cat > justfile << 'EOF'
# ClipSync development commands for macOS

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

# Profile with Instruments (macOS-specific)
profile:
    cargo build --profile release
    xcrun xctrace record --template "Time Profiler" --launch ./target/release/clipsync -- --help

# Quick install locally
install:
    cargo install --path . --force

# Build universal binary (Intel + Apple Silicon)
universal:
    cargo build --release --target x86_64-apple-darwin
    cargo build --release --target aarch64-apple-darwin
    lipo -create -output target/release/clipsync-universal \
        target/x86_64-apple-darwin/release/clipsync \
        target/aarch64-apple-darwin/release/clipsync

# Cross-compile for Linux
linux:
    cross build --release --target x86_64-unknown-linux-gnu
    cross build --release --target aarch64-unknown-linux-gnu

# Clean everything
clean:
    cargo clean
    rm -rf target
EOF

# Setup environment variables
echo ""
echo "🔧 Setting up environment..."
if [[ "$SHELL" == *"zsh"* ]]; then
    SHELL_RC="$HOME/.zshrc"
elif [[ "$SHELL" == *"bash"* ]]; then
    SHELL_RC="$HOME/.bash_profile"
else
    SHELL_RC="$HOME/.profile"
fi

# Add environment variables if not already present
if ! grep -q "# ClipSync development" "$SHELL_RC" 2>/dev/null; then
    cat >> "$SHELL_RC" << 'EOF'

# ClipSync development environment
export PKG_CONFIG_PATH="$(brew --prefix)/lib/pkgconfig:$PKG_CONFIG_PATH"
export OPENSSL_DIR="$(brew --prefix openssl)"
export OPENSSL_NO_VENDOR=1
EOF
    echo "✅ Added environment variables to $SHELL_RC"
fi

echo ""
echo "✅ Setup complete!"
echo ""
echo "🎉 macOS optimizations applied:"
echo "   • Native CPU optimization for $ARCH"
echo "   • Homebrew OpenSSL integration"
echo "   • Universal binary support"
echo "   • Cross-compilation to Linux"
echo ""
echo "⚡ Quick commands:"
echo "   just dev       - Fast development build (~3-10s)"
echo "   just watch     - Auto-rebuild on changes"
echo "   just check     - Type checking only (~1-3s)"
echo "   just universal - Build universal binary (Intel + ARM)"
echo "   just linux     - Cross-compile for Linux"
echo ""
echo "🔥 Expected build times on macOS:"
if [[ "$ARCH" == "arm64" ]]; then
    echo "   • Development build: 3-8 seconds (Apple Silicon is fast!)"
    echo "   • Clean release build: 15-30 seconds"
    echo "   • Incremental build: 0.5-2 seconds"
else
    echo "   • Development build: 5-12 seconds (Intel)"
    echo "   • Clean release build: 20-40 seconds"
    echo "   • Incremental build: 1-3 seconds"
fi
echo ""
echo "🍎 macOS-specific features:"
echo "   • Instruments profiling support"
echo "   • Universal binary builds"
echo "   • Native macOS clipboard integration"
echo ""
echo "💡 Restart your terminal or run: source $SHELL_RC"