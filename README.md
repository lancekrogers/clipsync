# 📋 ClipSync

> **Secure, Real-Time Clipboard Synchronization Across Your Devices**

ClipSync is a fast, secure, and easy-to-use clipboard synchronization service that keeps your clipboard in sync across macOS and Linux devices. Built in Rust for maximum performance and security.

```
Copy on your laptop → Instantly available on your desktop → Paste anywhere
```

[![Build Status](https://github.com/yourusername/clipsync/workflows/CI/badge.svg)](https://github.com/yourusername/clipsync/actions)
[![Security Audit](https://github.com/yourusername/clipsync/workflows/Security/badge.svg)](https://github.com/yourusername/clipsync/actions)
[![License](https://img.shields.io/badge/license-MIT%2FApache-blue.svg)](LICENSE)

## ✨ Key Features

🔐 **Secure by Default**
- SSH key authentication - only your devices connect
- End-to-end encryption with AES-256-GCM
- No cloud servers - direct peer-to-peer communication
- Encrypted local clipboard history

🚀 **Lightning Fast**
- Sub-500ms sync latency for text
- Built in Rust for maximum performance
- Automatic LAN discovery via mDNS
- Background service with minimal resource usage

📱 **Cross-Platform**
- **macOS**: Full NSPasteboard support with native notifications
- **Linux**: X11 and Wayland support with primary selection sync
- Support for text, RTF, and images up to 5MB

⚡ **Smart Features**
- 20-item encrypted clipboard history with search
- Global hotkeys for instant access (`Ctrl+Shift+V` for history)
- Interactive terminal UI for history browsing
- Automatic reconnection and network resilience

🛠️ **Developer Friendly**
- Comprehensive CLI with `--help` for every command
- JSON/TOML configuration with validation
- Structured logging and status reporting
- Built-in troubleshooting tools

## 🚀 Quick Start

### 1️⃣ Install ClipSync

**macOS (Homebrew)**
```bash
brew install clipsync
```

**Linux (Binary)**
```bash
curl -L https://github.com/yourusername/clipsync/releases/latest/download/clipsync-linux-x86_64.tar.gz | tar xz
sudo mv clipsync /usr/local/bin/
```

**From Source**
```bash
git clone https://github.com/yourusername/clipsync.git
cd clipsync && cargo install --path .
```

### 2️⃣ Generate SSH Key (if needed)
```bash
# ClipSync uses SSH keys for secure authentication
ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_clipsync
```

### 3️⃣ Start on Both Devices
```bash
# Device 1 & 2: Start the service
clipsync start

# Check connection status
clipsync status
```

### 4️⃣ Connect Devices
```bash
# On device 1: Add device 2's public key
clipsync auth add ~/.ssh/id_ed25519_clipsync.pub --name "laptop"

# On device 2: Add device 1's public key  
clipsync auth add ~/.ssh/id_ed25519_clipsync.pub --name "desktop"
```

### 5️⃣ Test It Out! 
Copy something on one device and paste on another. Use `Ctrl+Shift+V` to see clipboard history!

> 📖 **Need help?** See the [Installation Guide](docs/INSTALL.md) for detailed setup instructions.

## 💻 Command Reference

### Service Management
```bash
clipsync start [--foreground]    # Start daemon (background by default)
clipsync stop                    # Stop daemon
clipsync status                  # Show service status and connections
clipsync restart                 # Restart the service
```

### Clipboard Operations
```bash
clipsync copy "Hello World"      # Copy text to clipboard
clipsync paste                   # Get current clipboard content
clipsync sync                    # Force immediate sync across devices
clipsync clear                   # Clear clipboard content
```

### History Management
```bash
clipsync history                 # Show recent clipboard history
clipsync history --limit 10      # Show last 10 items
clipsync history --interactive   # Interactive history picker
clipsync history --search "text" # Search clipboard history
```

### Peer Management  
```bash
clipsync peers                   # List connected devices
clipsync peers --discover        # Scan for devices on network
clipsync auth add <public_key>   # Add authorized device
clipsync auth list               # List authorized keys
clipsync auth remove <key_id>    # Remove device access
```

### Configuration
```bash
clipsync config show            # Display current configuration
clipsync config init            # Create default config file
clipsync config validate        # Check configuration validity
clipsync config edit            # Open config in editor
```

### Troubleshooting
```bash
clipsync doctor                 # Run connectivity diagnostics
clipsync logs                   # Show recent log entries
clipsync version                # Show version information
clipsync --help                 # Show comprehensive help
```

## ⌨️ Global Hotkeys

| Hotkey | Action |
|--------|--------|
| `Ctrl+Shift+V` | Show clipboard history picker |
| `Ctrl+Shift+C` | Copy to secondary clipboard |
| `Ctrl+Shift+S` | Force sync now |
| `Ctrl+Shift+[` | Previous history item |
| `Ctrl+Shift+]` | Next history item |

> **macOS**: Replace `Ctrl` with `Cmd` • **Customizable**: Edit hotkeys in config.toml

## ⚙️ Configuration

ClipSync uses a TOML configuration file located at:
- **Linux**: `~/.config/clipsync/config.toml`
- **macOS**: `~/Library/Application Support/clipsync/config.toml`

### Quick Configuration
```bash
# Generate default config
clipsync config init

# Edit configuration
clipsync config edit

# Validate settings
clipsync config validate
```

### Example Configuration
```toml
# Network settings
listen_addr = ":8484"                    # Server listen address
advertise_name = "my-laptop-clipsync"    # mDNS service name

[auth]
ssh_key = "~/.ssh/id_ed25519"           # SSH private key for auth
authorized_keys = "~/.config/clipsync/authorized_keys"  # Authorized peers

[clipboard]
max_size = 5_242_880                     # Max payload size (5MB)
sync_primary = true                      # Sync X11 primary selection (Linux)
history_size = 20                        # Number of history items to keep

[hotkeys]
show_history = "Ctrl+Shift+V"           # Show clipboard history
toggle_sync = "Ctrl+Shift+Cmd+C"        # Toggle sync on/off
cycle_prev = "Ctrl+Shift+["             # Previous history item

[security]
encryption = "aes-256-gcm"              # Encryption algorithm
compression = "zstd"                    # Compression for large payloads

# Logging level: trace, debug, info, warn, error
log_level = "info"
```

> 📚 **Full reference**: See [Configuration Guide](docs/CONFIG.md) for all available options

## 🔧 Troubleshooting

### Common Issues

**Connection Problems**
```bash
# Run diagnostics
clipsync doctor

# Check if devices can see each other
clipsync peers --discover

# Verify SSH keys are set up correctly
clipsync auth list
```

**Service Not Starting**
```bash
# Check service status
clipsync status

# View recent logs
clipsync logs

# Start in foreground for debugging
clipsync start --foreground
```

**Sync Not Working**
```bash
# Force immediate sync
clipsync sync

# Check configuration
clipsync config validate

# Restart service
clipsync restart
```

> 🩺 **Need more help?** See the [Troubleshooting Guide](docs/TROUBLESHOOTING.md) for detailed solutions

## 🏗️ Development

### Quick Development Setup
```bash
# Clone and build
git clone https://github.com/yourusername/clipsync.git
cd clipsync
cargo build

# Run tests
cargo test

# Start with debug logging
RUST_LOG=debug cargo run -- start --foreground
```

### Requirements
- **Rust**: 1.75+ (2021 edition)
- **Platform deps**:
  - **macOS**: Xcode Command Line Tools
  - **Linux**: `libx11-dev libxcb-dev libssl-dev`

> 🔨 **Contributing?** Check out the [Developer Guide](CONTRIBUTING.md)

## 🏛️ Architecture

```
┌─────────────────┐    ┌─────────────────┐
│   Device A      │    │   Device B      │
│                 │    │                 │
│ ┌─────────────┐ │    │ ┌─────────────┐ │
│ │ Clipboard   │ │    │ │ Clipboard   │ │
│ │ Monitor     │ │    │ │ Monitor     │ │
│ └─────────────┘ │    │ └─────────────┘ │
│        │        │    │        │        │
│ ┌─────────────┐ │    │ ┌─────────────┐ │
│ │   History   │ │    │ │   History   │ │
│ │  Database   │ │    │ │  Database   │ │
│ │ (encrypted) │ │    │ │ (encrypted) │ │
│ └─────────────┘ │    │ └─────────────┘ │
│        │        │    │        │        │
│ ┌─────────────┐ │    │ ┌─────────────┐ │
│ │ Transport   │◄┼────┼►│ Transport   │ │
│ │  WebSocket  │ │    │ │  WebSocket  │ │
│ │   + SSH     │ │    │ │   + SSH     │ │
│ └─────────────┘ │    │ └─────────────┘ │
│        │        │    │        │        │
│ ┌─────────────┐ │    │ ┌─────────────┐ │
│ │ mDNS/DNS-SD │ │    │ │ mDNS/DNS-SD │ │
│ │ Discovery   │ │    │ │ Discovery   │ │
│ └─────────────┘ │    │ └─────────────┘ │
└─────────────────┘    └─────────────────┘
```

**Core Components:**
- 📋 **Clipboard Module**: Platform-specific clipboard access (NSPasteboard/X11/Wayland)
- 🔐 **Transport Layer**: WebSocket with SSH authentication and AES-256-GCM encryption
- 💾 **History Database**: SQLite with SQLCipher for encrypted local storage
- 🔍 **Service Discovery**: mDNS/DNS-SD for automatic LAN peer discovery
- ⌨️ **Hotkey System**: Global hotkey registration and handling
- 🖥️ **CLI Interface**: Comprehensive command-line interface with interactive features

## 🛡️ Security

ClipSync is built with security as a top priority:

🔑 **Authentication**
- SSH Ed25519 key pairs for device authentication
- No passwords or shared secrets
- Authorized keys file similar to SSH

🔒 **Encryption**
- AES-256-GCM for all network communication
- Perfect forward secrecy with session keys
- Encrypted local clipboard history database

🌐 **Network Security**
- Direct peer-to-peer communication (no cloud)
- WebSocket over TLS for transport security
- mDNS discovery limited to LAN only

🏠 **Privacy**
- All data stays on your devices
- No telemetry or data collection
- Open source for full transparency

> 🔒 **Security concerns?** See our [Security Guide](docs/SECURITY.md) or email security@clipsync.dev

## 📚 Documentation

| Guide | Description |
|-------|-------------|
| [Installation Guide](docs/INSTALL.md) | Detailed setup instructions for all platforms |
| [User Guide](docs/USER_GUIDE.md) | Complete user manual with tutorials |
| [Configuration Guide](docs/CONFIG.md) | Full configuration reference |
| [Troubleshooting](docs/TROUBLESHOOTING.md) | Common issues and solutions |
| [Developer Guide](CONTRIBUTING.md) | Development setup and contribution guidelines |
| [Security Guide](docs/SECURITY.md) | Security model and best practices |
| [API Reference](docs/API.md) | Technical API documentation |

## 🤝 Contributing

We welcome contributions! Please see our [Contributing Guide](CONTRIBUTING.md) for:
- Development setup instructions
- Code style guidelines  
- Testing requirements
- Pull request process

## 📄 License

ClipSync is dual-licensed under your choice of:
- **MIT License** ([LICENSE-MIT](LICENSE-MIT))
- **Apache License 2.0** ([LICENSE-APACHE](LICENSE-APACHE))

## 🙏 Acknowledgments

Built with excellent open-source projects:
- [Tokio](https://tokio.rs/) - Async runtime
- [Serde](https://serde.rs/) - Serialization
- [SQLCipher](https://www.zetetic.net/sqlcipher/) - Encrypted database
- [tungstenite](https://github.com/snapview/tungstenite-rs) - WebSocket implementation
- [zeroize](https://github.com/RustCrypto/utils/tree/master/zeroize) - Secure memory handling

---

**Made with ❤️ and 🦀 Rust** • [Report Issues](https://github.com/yourusername/clipsync/issues) • [Join Discussions](https://github.com/yourusername/clipsync/discussions)