#!/bin/bash
set -e

echo "ClipSync Installation Test Script"
echo "================================="

# Check OS
if [[ "$OSTYPE" == "darwin"* ]]; then
    OS="macos"
elif [[ "$OSTYPE" == "linux-gnu"* ]]; then
    if command -v pacman &> /dev/null; then
        OS="arch"
    else
        OS="linux"
    fi
else
    echo "Unsupported OS: $OSTYPE"
    exit 1
fi

echo "Detected OS: $OS"

# Function to test basic functionality
test_basic() {
    echo -e "\n1. Testing binary execution..."
    ./target/release/clipsync --version
    
    echo -e "\n2. Testing help command..."
    ./target/release/clipsync --help
    
    echo -e "\n3. Testing config generation..."
    ./target/release/clipsync config init > test_config.toml
    echo "Generated config:"
    head -20 test_config.toml
    rm -f test_config.toml
    
    echo -e "\n4. Testing setup wizard (non-interactive)..."
    echo -e "n\n" | ./target/release/clipsync setup || true
}

# Function to test installation
test_install() {
    echo -e "\n5. Testing installation process..."
    
    if [[ "$OS" == "macos" ]]; then
        # Test macOS installation
        echo "Testing macOS installation..."
        
        # Check dependencies
        echo "Checking dependencies..."
        command -v brew &> /dev/null && echo "✓ Homebrew installed" || echo "✗ Homebrew not found"
        
        # Test binary copy (without sudo)
        echo "Would copy binary to: /usr/local/bin/clipsync"
        
        # Test LaunchAgent
        echo "Would install LaunchAgent to: ~/Library/LaunchAgents/com.clipsync.plist"
        
    elif [[ "$OS" == "arch" ]]; then
        # Test Arch installation
        echo "Testing Arch Linux installation..."
        
        # Check dependencies
        echo "Checking dependencies..."
        pacman -Q libx11 &> /dev/null && echo "✓ libx11 installed" || echo "✗ libx11 not found"
        pacman -Q openssl &> /dev/null && echo "✓ openssl installed" || echo "✗ openssl not found"
        
        # Test systemd service
        echo "Would install systemd service to: /etc/systemd/system/clipsync.service"
    fi
}

# Function to test clipboard functionality
test_clipboard() {
    echo -e "\n6. Testing clipboard functionality..."
    
    # First create a test config
    ./target/release/clipsync config init > test_config.toml
    
    # Start the service in test mode
    echo "Starting ClipSync in test mode..."
    timeout 5s ./target/release/clipsync start --config test_config.toml &
    CLIPSYNC_PID=$!
    
    sleep 2
    
    # Test clipboard operations
    if [[ "$OS" == "macos" ]]; then
        echo "test clipboard data" | pbcopy
        echo "Copied test data to clipboard"
    elif [[ "$OS" == "arch" ]]; then
        if command -v xclip &> /dev/null; then
            echo "test clipboard data" | xclip -selection clipboard
            echo "Copied test data to clipboard"
        fi
    fi
    
    # Kill the test process
    kill $CLIPSYNC_PID 2>/dev/null || true
}

# Run tests
test_basic
test_install
# test_clipboard  # Uncomment to test clipboard (requires X11 on Linux)

echo -e "\n================================="
echo "Installation test completed!"
echo "Please review the output for any issues."