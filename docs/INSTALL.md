# 📥 ClipSync Installation Guide

This guide covers installation methods for all supported platforms, from package managers to building from source.

## 🚀 Quick Install

### macOS

**Homebrew (Recommended)**
```bash
brew install clipsync
```

**Direct Download**
```bash
curl -L https://github.com/lancekrogers/clipsync/releases/latest/download/clipsync-macos-x86_64.tar.gz | tar xz
sudo mv clipsync /usr/local/bin/
```

### Linux

**Arch Linux (AUR)**
```bash
yay -S clipsync
# or
paru -S clipsync
```

**Ubuntu/Debian**
```bash
curl -L https://github.com/lancekrogers/clipsync/releases/latest/download/clipsync-linux-x86_64.deb -o clipsync.deb
sudo dpkg -i clipsync.deb
```

**Direct Download**
```bash
curl -L https://github.com/lancekrogers/clipsync/releases/latest/download/clipsync-linux-x86_64.tar.gz | tar xz
sudo mv clipsync /usr/local/bin/
```

## 📋 Prerequisites Check

Before installing, verify your system meets the requirements:

### System Requirements

**macOS**
- macOS 10.15 (Catalina) or later
- Intel x86_64 or Apple Silicon (M1/M2)
- 50MB free disk space

**Linux**
- Any modern Linux distribution (Ubuntu 18.04+, Fedora 30+, etc.)
- X11 or Wayland display server
- glibc 2.27+ or musl libc
- 50MB free disk space

### Runtime Dependencies

**macOS**: No additional dependencies needed

**Linux**: Install required system libraries:
```bash
# Ubuntu/Debian
sudo apt update
sudo apt install libx11-6 libxcb1 libssl3

# Fedora/RHEL
sudo dnf install libX11 libxcb openssl

# Arch Linux
sudo pacman -S libx11 libxcb openssl
```

## 🔧 Detailed Installation Instructions

### macOS Installation

#### Method 1: Homebrew (Recommended)

1. **Install Homebrew** (if not already installed):
   ```bash
   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
   ```

2. **Install ClipSync**:
   ```bash
   brew tap lancekrogers/clipsync
   brew install clipsync
   ```

3. **Verify Installation**:
   ```bash
   clipsync --version
   ```

4. **Set up Launch Agent** (optional, for auto-start):
   ```bash
   brew services start clipsync
   ```

#### Method 2: Direct Binary Installation

1. **Download the binary**:
   ```bash
   cd ~/Downloads
   curl -L https://github.com/lancekrogers/clipsync/releases/latest/download/clipsync-macos-universal.tar.gz -o clipsync.tar.gz
   ```

2. **Extract and install**:
   ```bash
   tar -xzf clipsync.tar.gz
   sudo mv clipsync /usr/local/bin/
   sudo chmod +x /usr/local/bin/clipsync
   ```

3. **Create config directory**:
   ```bash
   mkdir -p ~/Library/Application\ Support/clipsync
   ```

4. **Verify installation**:
   ```bash
   clipsync --version
   ```

### Linux Installation

#### Method 1: Package Manager

**Arch Linux (AUR)**:
```bash
# Using yay
yay -S clipsync

# Using paru  
paru -S clipsync

# Manual AUR installation
git clone https://aur.archlinux.org/clipsync.git
cd clipsync
makepkg -si
```

**Ubuntu/Debian**:
```bash
# Download and install .deb package
wget https://github.com/lancekrogers/clipsync/releases/latest/download/clipsync_1.0.0_amd64.deb
sudo dpkg -i clipsync_1.0.0_amd64.deb

# Fix any dependency issues
sudo apt-get install -f
```

**Fedora/RHEL**:
```bash
# Download and install .rpm package
wget https://github.com/lancekrogers/clipsync/releases/latest/download/clipsync-1.0.0-1.x86_64.rpm
sudo rpm -i clipsync-1.0.0-1.x86_64.rpm
```

#### Method 2: Direct Binary Installation

1. **Install dependencies**:
   ```bash
   # Ubuntu/Debian
   sudo apt update
   sudo apt install libx11-6 libxcb1 libssl3
   
   # Fedora/RHEL
   sudo dnf install libX11 libxcb openssl
   
   # Arch Linux
   sudo pacman -S libx11 libxcb openssl
   ```

2. **Download and install binary**:
   ```bash
   curl -L https://github.com/lancekrogers/clipsync/releases/latest/download/clipsync-linux-x86_64.tar.gz | tar xz
   sudo mv clipsync /usr/local/bin/
   sudo chmod +x /usr/local/bin/clipsync
   ```

3. **Create config directory**:
   ```bash
   mkdir -p ~/.config/clipsync
   mkdir -p ~/.local/share/clipsync
   ```

4. **Verify installation**:
   ```bash
   clipsync --version
   ```

## 🏗️ Building from Source

### Prerequisites for Building

**Required Tools**:
- Rust 1.75.0 or later
- Git
- pkg-config

**macOS Build Dependencies**:
```bash
# Install Xcode Command Line Tools
xcode-select --install

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

**Linux Build Dependencies**:
```bash
# Ubuntu/Debian
sudo apt update
sudo apt install build-essential pkg-config libx11-dev libxcb-dev libssl-dev git curl

# Fedora/RHEL
sudo dnf groupinstall "Development Tools"
sudo dnf install pkg-config libX11-devel libxcb-devel openssl-devel git curl

# Arch Linux
sudo pacman -S base-devel pkg-config libx11 libxcb openssl git curl

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

### Build Process

1. **Clone the repository**:
   ```bash
   git clone https://github.com/lancekrogers/clipsync.git
   cd clipsync
   ```

2. **Build the project**:
   ```bash
   # Debug build (faster compilation, larger binary)
   cargo build
   
   # Release build (optimized, smaller binary)
   cargo build --release
   ```

3. **Run tests** (optional but recommended):
   ```bash
   cargo test
   ```

4. **Install locally**:
   ```bash
   cargo install --path .
   ```

5. **Verify installation**:
   ```bash
   clipsync --version
   ```

### Cross-Compilation

To build for different architectures:

```bash
# Install cross-compilation targets
rustup target add x86_64-apple-darwin
rustup target add aarch64-apple-darwin
rustup target add x86_64-unknown-linux-gnu

# Build for specific targets
cargo build --release --target x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-unknown-linux-gnu
```

## ⚙️ Post-Installation Setup

### 1. Generate SSH Keys (Required)

ClipSync uses SSH keys for secure device authentication:

```bash
# Generate a new Ed25519 key pair for ClipSync
ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_clipsync -C "clipsync-$(hostname)"

# Or use an existing SSH key
# ClipSync will use ~/.ssh/id_ed25519 by default
```

### 2. Initialize Configuration

```bash
# Create default configuration file
clipsync config init

# Edit configuration (opens in your default editor)
clipsync config edit

# Validate configuration
clipsync config validate
```

### 3. Set Up Service (Optional)

**macOS (launchd)**:
```bash
# Install service for current user
clipsync service install --user

# Start service
clipsync service start

# Enable auto-start on login
clipsync service enable
```

**Linux (systemd)**:
```bash
# Install service for current user
clipsync service install --user

# Start service
systemctl --user start clipsync

# Enable auto-start on login
systemctl --user enable clipsync
```

### 4. Test Installation

```bash
# Start ClipSync in foreground (for testing)
clipsync start --foreground

# In another terminal, check status
clipsync status

# Copy some text and check clipboard
echo "Hello ClipSync!" | clipsync copy
clipsync paste
```

## 🔍 Verification Steps

After installation, verify everything is working:

### 1. Basic Functionality
```bash
# Check version
clipsync --version

# Show help
clipsync --help

# Validate configuration
clipsync config validate
```

### 2. Service Functionality
```bash
# Start the service
clipsync start

# Check status
clipsync status

# Test clipboard operations
echo "test" | clipsync copy
clipsync paste

# Stop the service
clipsync stop
```

### 3. Network Discovery (Multi-Device)
```bash
# Start service on both devices
clipsync start

# Check for discoverable peers
clipsync peers --discover

# View service logs
clipsync logs
```

## 🚨 Troubleshooting Installation

### Common Issues

**Permission Denied (macOS)**
```bash
# If binary is blocked by Gatekeeper
sudo xattr -d com.apple.quarantine /usr/local/bin/clipsync

# Or allow in System Preferences > Security & Privacy
```

**Missing Dependencies (Linux)**
```bash
# Check which libraries are missing
ldd $(which clipsync)

# Install missing dependencies
sudo apt install libx11-6 libxcb1 libssl3  # Ubuntu/Debian
sudo dnf install libX11 libxcb openssl      # Fedora/RHEL
```

**Compilation Errors (Source Build)**
```bash
# Update Rust toolchain
rustup update

# Clean build cache
cargo clean

# Update dependencies
cargo update
```

**Service Won't Start**
```bash
# Check for configuration errors
clipsync config validate

# Start in foreground to see errors
clipsync start --foreground

# Check system logs
journalctl --user -u clipsync  # Linux
log show --predicate 'process == "clipsync"' --info  # macOS
```

### Getting Help

If you encounter issues:

1. **Check the logs**: `clipsync logs`
2. **Run diagnostics**: `clipsync doctor`
3. **Validate config**: `clipsync config validate`
4. **Check the [Troubleshooting Guide](TROUBLESHOOTING.md)**
5. **Search [existing issues](https://github.com/lancekrogers/clipsync/issues)**
6. **Create a [new issue](https://github.com/lancekrogers/clipsync/issues/new)**

## 🔄 Updating ClipSync

### Package Manager Updates

**Homebrew (macOS)**:
```bash
brew update
brew upgrade clipsync
```

**AUR (Arch Linux)**:
```bash
yay -Syu clipsync
```

**Manual Updates**:
```bash
# Download latest release
# Follow the same installation steps as above
```

### Source Updates

```bash
cd clipsync
git pull origin main
cargo build --release
cargo install --path .
```

## 🗑️ Uninstallation

### Remove ClipSync

**Homebrew (macOS)**:
```bash
brew uninstall clipsync
```

**Package Manager (Linux)**:
```bash
# AUR
yay -Rns clipsync

# Debian/Ubuntu
sudo apt remove clipsync

# Fedora/RHEL
sudo rpm -e clipsync
```

**Manual Removal**:
```bash
# Remove binary
sudo rm /usr/local/bin/clipsync

# Remove service files (if installed)
clipsync service uninstall

# Remove configuration and data (optional)
rm -rf ~/.config/clipsync           # Linux
rm -rf ~/Library/Application\ Support/clipsync  # macOS
rm -rf ~/.local/share/clipsync      # Linux
```

---

**Next Steps**: After installation, see the [User Guide](USER_GUIDE.md) to learn how to use ClipSync effectively.