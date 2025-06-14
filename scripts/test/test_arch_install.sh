#!/bin/bash
# Arch Linux Installation Test Script
# Run this on your Arch Linux system

set -e

echo "ClipSync Arch Linux Installation Test"
echo "===================================="

# Check if running on Arch
if ! command -v pacman &> /dev/null; then
    echo "This script is designed for Arch Linux"
    exit 1
fi

# Check dependencies
echo -e "\n1. Checking dependencies..."
deps=("libx11" "libxcb" "openssl")
missing_deps=()

for dep in "${deps[@]}"; do
    if pacman -Q "$dep" &> /dev/null; then
        echo "✓ $dep installed"
    else
        echo "✗ $dep not installed"
        missing_deps+=("$dep")
    fi
done

if [ ${#missing_deps[@]} -ne 0 ]; then
    echo -e "\nInstall missing dependencies with:"
    echo "sudo pacman -S ${missing_deps[*]}"
fi

# Check display server
echo -e "\n2. Checking display server..."
if [ -n "$WAYLAND_DISPLAY" ]; then
    echo "✓ Wayland detected"
elif [ -n "$DISPLAY" ]; then
    echo "✓ X11 detected"
else
    echo "✗ No display server detected"
fi

# Build the project
echo -e "\n3. Building ClipSync..."
if [ ! -f "Cargo.toml" ]; then
    echo "Error: Run this script from the ClipSync project root"
    exit 1
fi

cargo build --release --target x86_64-unknown-linux-gnu

# Test the binary
echo -e "\n4. Testing binary..."
./target/x86_64-unknown-linux-gnu/release/clipsync --version

# Test clipboard functionality
echo -e "\n5. Testing clipboard access..."
if command -v xclip &> /dev/null; then
    echo "test data" | xclip -selection clipboard
    echo "✓ xclip test successful"
elif command -v wl-copy &> /dev/null; then
    echo "test data" | wl-copy
    echo "✓ wl-copy test successful"
else
    echo "✗ No clipboard tool found (install xclip for X11 or wl-clipboard for Wayland)"
fi

# Test systemd service
echo -e "\n6. Testing systemd service configuration..."
echo "Would install to: /etc/systemd/system/clipsync.service"
echo "Service file contents:"
cat scripts/clipsync.service

echo -e "\n===================================="
echo "Test completed!"
echo ""
echo "To install on Arch Linux:"
echo "1. sudo cp target/x86_64-unknown-linux-gnu/release/clipsync /usr/local/bin/"
echo "2. sudo cp scripts/clipsync.service /etc/systemd/system/"
echo "3. sudo systemctl daemon-reload"
echo "4. sudo systemctl enable --now clipsync"