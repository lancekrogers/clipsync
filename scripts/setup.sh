#!/bin/bash
set -e

# ClipSync universal setup script

echo "ClipSync Development Environment Setup"
echo "====================================="

# Detect operating system
if [[ "$OSTYPE" == "linux-gnu"* ]]; then
    if command -v pacman &> /dev/null; then
        echo "🐧 Detected Arch Linux"
        exec ./scripts/setup-arch.sh
    elif command -v apt &> /dev/null; then
        echo "🐧 Detected Ubuntu/Debian"
        echo "Using generic Linux setup..."
        # Basic Ubuntu setup
        sudo apt update
        sudo apt install -y build-essential curl pkg-config libssl-dev libx11-dev libxcb1-dev libxrandr-dev libdbus-1-dev
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source ~/.cargo/env
        cargo install just cargo-watch
        echo "✅ Basic Linux setup complete. For better performance, see scripts/setup-arch.sh"
    else
        echo "❓ Unknown Linux distribution"
        echo "Please install Rust manually: https://rustup.rs/"
        exit 1
    fi
elif [[ "$OSTYPE" == "darwin"* ]]; then
    echo "🍎 Detected macOS"
    exec ./scripts/setup-macos.sh
elif [[ "$OSTYPE" == "msys" || "$OSTYPE" == "cygwin" ]]; then
    echo "🪟 Windows detected"
    echo "Please use WSL2 with Ubuntu or install Rust for Windows:"
    echo "https://forge.rust-lang.org/infra/channel-layout.html#windows"
    exit 1
else
    echo "❓ Unknown operating system: $OSTYPE"
    echo "Please install Rust manually: https://rustup.rs/"
    exit 1
fi