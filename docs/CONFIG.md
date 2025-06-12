# ⚙️ ClipSync Configuration Reference

Complete reference for all ClipSync configuration options, with examples and best practices.

## 📍 Configuration File Location

ClipSync looks for configuration in the following locations:

**macOS:**
```
~/Library/Application Support/clipsync/config.toml
```

**Linux:**
```
~/.config/clipsync/config.toml
```

**Custom Location:**
```bash
clipsync --config /path/to/custom/config.toml start
```

**Environment Variable:**
```bash
export CLIPSYNC_CONFIG=/path/to/config.toml
```

## 🔧 Configuration Management

### Quick Configuration Commands

```bash
# Create default configuration file
clipsync config init

# Show current configuration
clipsync config show

# Edit configuration in default editor
clipsync config edit

# Validate configuration syntax and values
clipsync config validate

# Show configuration file location
clipsync config path
```

## 📋 Complete Configuration Reference

### Basic Example

```toml
# Minimal working configuration
listen_addr = ":8484"
advertise_name = "my-laptop-clipsync"

[auth]
ssh_key = "~/.ssh/id_ed25519"

[clipboard]
max_size = 5_242_880  # 5MB
history_size = 20
```

### Network Configuration

```toml
# Network listen address and port
listen_addr = ":8484"                    # Listen on all interfaces, port 8484
# listen_addr = "127.0.0.1:8484"        # Listen only on localhost
# listen_addr = "192.168.1.100:8484"    # Listen on specific IP

# mDNS service advertisement name
advertise_name = "hostname-clipsync"     # Default: hostname + "-clipsync"
# advertise_name = "johns-laptop"        # Custom name for easy identification

# Manual peer configuration (bypasses discovery)
[[peers]]
name = "desktop"
address = "192.168.1.50:8484"
public_key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI..."

[[peers]]
name = "server"
address = "10.0.0.100:8484"
public_key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI..."
```

#### Network Options Reference

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `listen_addr` | String | `":8484"` | Address and port to listen on |
| `advertise_name` | String | `"hostname-clipsync"` | mDNS service name |
| `timeout_connect` | Duration | `"30s"` | Connection timeout |
| `timeout_handshake` | Duration | `"10s"` | Handshake timeout |
| `keepalive_interval` | Duration | `"30s"` | Keep-alive ping interval |

### Authentication Configuration

```toml
[auth]
# SSH private key for authentication
ssh_key = "~/.ssh/id_ed25519"                    # Default SSH key
# ssh_key = "~/.ssh/id_ed25519_clipsync"        # ClipSync-specific key
# ssh_key = "/path/to/custom/key"               # Custom key location

# Authorized keys file (similar to SSH authorized_keys)
authorized_keys = "~/.config/clipsync/authorized_keys"

# Key verification settings
strict_host_key_checking = true                  # Reject unknown peers
allow_self_signed = false                        # Require proper SSH key signatures
```

#### Auth Options Reference

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `ssh_key` | String | `"~/.ssh/id_ed25519"` | Path to SSH private key |
| `authorized_keys` | String | `"~/.config/clipsync/authorized_keys"` | Authorized keys file |
| `strict_host_key_checking` | Boolean | `true` | Reject connections from unknown peers |
| `allow_self_signed` | Boolean | `false` | Allow self-signed certificates |
| `key_algorithm` | String | `"ed25519"` | Preferred key algorithm |

### Clipboard Configuration

```toml
[clipboard]
# Maximum clipboard content size (in bytes)
max_size = 5_242_880                     # 5MB default
# max_size = 1_048_576                   # 1MB for slower networks
# max_size = 52_428_800                  # 50MB maximum allowed

# Clipboard history settings
history_size = 20                        # Number of items to keep in history
# history_size = 10                      # Smaller history for better performance
# history_size = 50                      # Larger history for power users

# History database location
history_db = "~/.local/share/clipsync/history.db"     # Linux default
# history_db = "~/Library/Application Support/clipsync/history.db"  # macOS default

# Platform-specific clipboard settings
sync_primary = true                      # Linux: sync X11 primary selection
# sync_primary = false                   # Disable primary selection sync

# Automatic sync settings
auto_sync = true                         # Enable automatic clipboard sync
sync_interval = "100ms"                  # Clipboard polling interval
# sync_interval = "500ms"                # Less frequent polling

# Content filtering
max_text_length = 1_000_000             # Maximum text length (characters)
allowed_mime_types = [                   # Allowed MIME types
    "text/plain",
    "text/html", 
    "text/rtf",
    "image/png",
    "image/jpeg",
    "image/tiff"
]

# Excluded applications (don't sync from these apps)
excluded_apps = [
    "1Password",                         # Password managers
    "Bitwarden",
    "KeePassXC",
    "LastPass"
]
```

#### Clipboard Options Reference

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `max_size` | Integer | `5242880` | Maximum clipboard content size (bytes) |
| `history_size` | Integer | `20` | Number of history items to keep |
| `history_db` | String | Platform default | History database file path |
| `sync_primary` | Boolean | `true` | Sync X11 primary selection (Linux only) |
| `auto_sync` | Boolean | `true` | Enable automatic clipboard synchronization |
| `sync_interval` | Duration | `"100ms"` | Clipboard polling interval |
| `max_text_length` | Integer | `1000000` | Maximum text length in characters |
| `allowed_mime_types` | Array | See above | Allowed MIME types for sync |
| `excluded_apps` | Array | `[]` | Applications to exclude from sync |

### Hotkey Configuration

```toml
[hotkeys]
# Enable/disable all hotkeys
enabled = true                           # Global hotkey toggle

# Individual hotkey bindings
show_history = "Ctrl+Shift+V"           # Show clipboard history picker
# show_history = "Cmd+Shift+V"          # macOS variant
# show_history = ""                     # Disable this hotkey

toggle_sync = "Ctrl+Shift+Cmd+C"        # Toggle sync on/off
# toggle_sync = "Ctrl+Alt+T"            # Alternative binding

force_sync = "Ctrl+Shift+S"             # Force immediate sync
cycle_prev = "Ctrl+Shift+["             # Previous history item
cycle_next = "Ctrl+Shift+]"             # Next history item
copy_secondary = "Ctrl+Shift+C"         # Copy to secondary clipboard

# Custom hotkeys
[hotkeys.custom]
clear_clipboard = "Ctrl+Shift+Delete"   # Clear clipboard
show_peers = "Ctrl+Shift+P"             # Show connected peers
```

#### Hotkey Format

Hotkeys use the following format:
- **Modifiers**: `Ctrl`, `Alt`, `Shift`, `Cmd` (macOS), `Win` (Windows)
- **Keys**: Letters (`A-Z`), numbers (`0-9`), function keys (`F1-F12`), special keys
- **Special Keys**: `Space`, `Tab`, `Enter`, `Escape`, `Delete`, `Backspace`, `[`, `]`, etc.
- **Combinations**: Join with `+`, e.g., `Ctrl+Shift+V`

#### Hotkey Options Reference

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | Boolean | `true` | Enable/disable all hotkeys |
| `show_history` | String | Platform default | Show clipboard history picker |
| `toggle_sync` | String | Platform default | Toggle clipboard sync on/off |
| `force_sync` | String | Platform default | Force immediate sync |
| `cycle_prev` | String | Platform default | Previous history item |
| `cycle_next` | String | Platform default | Next history item |
| `copy_secondary` | String | Platform default | Copy to secondary clipboard |

### Security Configuration

```toml
[security]
# Encryption settings
encryption = "aes-256-gcm"               # Encryption algorithm
# encryption = "chacha20-poly1305"       # Alternative algorithm

# Key derivation
key_derivation = "argon2id"              # Key derivation function
key_iterations = 100_000                 # KDF iterations

# Data compression (reduces network usage)
compression = "zstd"                     # Compression algorithm
# compression = "gzip"                   # Alternative compression
# compression = "none"                   # Disable compression

compression_level = 3                    # Compression level (1-9)
# compression_level = 1                  # Faster compression
# compression_level = 9                  # Better compression

# History encryption
encrypt_history = true                   # Encrypt local clipboard history
history_key_file = "~/.config/clipsync/history.key"  # Encryption key location

# Network security
require_tls = false                      # Require TLS for WebSocket (not needed with SSH)
verify_certificates = true              # Verify TLS certificates if TLS is used

# Audit and logging
audit_enabled = false                   # Enable audit logging
audit_file = "~/.config/clipsync/audit.log"  # Audit log location
audit_level = "info"                    # Audit detail level
```

#### Security Options Reference

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `encryption` | String | `"aes-256-gcm"` | Encryption algorithm |
| `key_derivation` | String | `"argon2id"` | Key derivation function |
| `key_iterations` | Integer | `100000` | KDF iterations |
| `compression` | String | `"zstd"` | Compression algorithm |
| `compression_level` | Integer | `3` | Compression level (1-9) |
| `encrypt_history` | Boolean | `true` | Encrypt local history database |
| `history_key_file` | String | Platform default | History encryption key file |
| `require_tls` | Boolean | `false` | Require TLS for connections |
| `verify_certificates` | Boolean | `true` | Verify TLS certificates |
| `audit_enabled` | Boolean | `false` | Enable audit logging |
| `audit_file` | String | Platform default | Audit log file path |

### Logging Configuration

```toml
[logging]
# Log level (trace, debug, info, warn, error)
level = "info"                          # Default log level
# level = "debug"                       # Verbose logging for troubleshooting
# level = "warn"                        # Minimal logging

# Log output destinations
console = true                          # Log to console/stdout
file = false                           # Log to file
# file = true                          # Enable file logging

# Log file settings (if file = true)
log_file = "~/.config/clipsync/clipsync.log"  # Log file location
max_size = "10MB"                       # Maximum log file size
max_files = 5                           # Number of log files to keep
compress = true                         # Compress rotated log files

# Log format
format = "text"                         # Log format (text, json)
# format = "json"                       # JSON format for log aggregation

timestamp = true                        # Include timestamps
colors = true                          # Colorize console output (if supported)

# Module-specific log levels
[logging.modules]
"clipsync::transport" = "debug"         # Verbose transport logging
"clipsync::clipboard" = "info"          # Standard clipboard logging
"clipsync::auth" = "warn"              # Minimal auth logging (security)
```

#### Logging Options Reference

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `level` | String | `"info"` | Global log level |
| `console` | Boolean | `true` | Log to console output |
| `file` | Boolean | `false` | Log to file |
| `log_file` | String | Platform default | Log file path |
| `max_size` | String | `"10MB"` | Maximum log file size |
| `max_files` | Integer | `5` | Number of rotated files to keep |
| `compress` | Boolean | `true` | Compress rotated log files |
| `format` | String | `"text"` | Log format (text or json) |
| `timestamp` | Boolean | `true` | Include timestamps in logs |
| `colors` | Boolean | `true` | Colorize console output |

### Performance Configuration

```toml
[performance]
# Threading and concurrency
worker_threads = 4                      # Number of worker threads
# worker_threads = 0                    # Auto-detect (CPU count)

# Memory settings
max_memory_usage = "100MB"              # Maximum memory usage
buffer_size = 65536                     # I/O buffer size (64KB)
# buffer_size = 32768                   # Smaller buffer for low memory

# Network performance
tcp_nodelay = true                      # Disable Nagle's algorithm
tcp_keepalive = true                    # Enable TCP keepalive
socket_buffer_size = 262144             # Socket buffer size (256KB)

# Clipboard polling optimization
adaptive_polling = true                 # Adjust polling based on activity
min_poll_interval = "50ms"              # Minimum polling interval
max_poll_interval = "1s"                # Maximum polling interval

# Background task intervals
discovery_interval = "30s"              # Peer discovery interval
health_check_interval = "60s"           # Connection health check interval
cleanup_interval = "300s"               # Database cleanup interval
```

#### Performance Options Reference

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `worker_threads` | Integer | `4` | Number of async worker threads |
| `max_memory_usage` | String | `"100MB"` | Maximum memory usage limit |
| `buffer_size` | Integer | `65536` | I/O buffer size in bytes |
| `tcp_nodelay` | Boolean | `true` | Disable Nagle's algorithm |
| `tcp_keepalive` | Boolean | `true` | Enable TCP keepalive |
| `socket_buffer_size` | Integer | `262144` | Socket buffer size in bytes |
| `adaptive_polling` | Boolean | `true` | Adaptive clipboard polling |
| `min_poll_interval` | Duration | `"50ms"` | Minimum polling interval |
| `max_poll_interval` | Duration | `"1s"` | Maximum polling interval |

## 📝 Configuration Examples

### Example 1: Minimal Configuration

```toml
# ~/.config/clipsync/config.toml
# Minimal setup for basic clipboard sync

listen_addr = ":8484"
advertise_name = "my-device"

[auth]
ssh_key = "~/.ssh/id_ed25519"

[clipboard]
max_size = 5_242_880
history_size = 20
```

### Example 2: High-Security Configuration

```toml
# Security-focused configuration
listen_addr = "127.0.0.1:8484"          # Only localhost
advertise_name = "secure-workstation"

[auth]
ssh_key = "~/.ssh/id_ed25519_clipsync"   # Dedicated key
strict_host_key_checking = true
allow_self_signed = false

[clipboard]
max_size = 1_048_576                     # 1MB limit
history_size = 10                        # Limited history
excluded_apps = ["1Password", "Bitwarden", "KeePassXC"]

[security]
encryption = "aes-256-gcm"
compression = "zstd"
encrypt_history = true
audit_enabled = true

[logging]
level = "warn"                           # Minimal logging
file = true
[logging.modules]
"clipsync::auth" = "info"               # Audit auth events
```

### Example 3: High-Performance Configuration

```toml
# Performance-optimized configuration
listen_addr = ":8484"
advertise_name = "workstation-pro"

[auth]
ssh_key = "~/.ssh/id_ed25519"

[clipboard]
max_size = 52_428_800                   # 50MB for large content
history_size = 50                       # Large history
sync_interval = "50ms"                  # Fast polling

[security]
compression = "zstd"
compression_level = 1                   # Fast compression

[performance]
worker_threads = 8                      # More threads
buffer_size = 131072                    # Larger buffers (128KB)
tcp_nodelay = true
adaptive_polling = true

[logging]
level = "error"                         # Minimal logging overhead
console = true
file = false
```

### Example 4: Multi-Device Office Setup

```toml
# Configuration for office environment with multiple devices
listen_addr = ":8484"
advertise_name = "johns-laptop"

[auth]
ssh_key = "~/.ssh/id_ed25519_clipsync"
authorized_keys = "~/.config/clipsync/office_authorized_keys"

# Manually configure known office devices
[[peers]]
name = "johns-desktop"
address = "192.168.1.50:8484"
public_key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI..."

[[peers]]
name = "meeting-room-pc"
address = "192.168.1.75:8484"
public_key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI..."

[clipboard]
max_size = 10_485_760                   # 10MB for presentations
history_size = 30
excluded_apps = ["1Password", "Slack"]  # Don't sync passwords or messages

[hotkeys]
show_history = "Ctrl+Alt+V"            # Avoid conflicts with other apps
toggle_sync = "Ctrl+Alt+T"

[security]
compression = "zstd"
audit_enabled = true                    # Track clipboard usage

[logging]
level = "info"
file = true
max_size = "50MB"
```

### Example 5: Developer Configuration

```toml
# Configuration optimized for software development
listen_addr = ":8484"
advertise_name = "dev-machine"

[auth]
ssh_key = "~/.ssh/id_ed25519_clipsync"

[clipboard]
max_size = 20_971_520                   # 20MB for large code files
history_size = 100                      # Extensive history for code snippets
sync_interval = "100ms"
allowed_mime_types = [
    "text/plain",
    "text/html",
    "text/x-shellscript",
    "application/json",
    "application/xml"
]

[hotkeys]
show_history = "Ctrl+Shift+H"          # Convenient for coding
force_sync = "Ctrl+Shift+S"
[hotkeys.custom]
copy_git_status = "Ctrl+Shift+G"       # Custom hotkey for git status

[security]
compression = "zstd"
compression_level = 5                   # Good compression for code

[logging]
level = "debug"                         # Verbose logging for development
file = true
format = "text"
[logging.modules]
"clipsync::transport" = "debug"
"clipsync::clipboard" = "info"
```

## 🔧 Configuration Validation

### Validation Commands

```bash
# Validate current configuration
clipsync config validate

# Validate specific config file
clipsync config validate --config /path/to/config.toml

# Show validation details
clipsync config validate --verbose
```

### Common Validation Errors

**Invalid TOML Syntax:**
```
Error: Invalid TOML syntax at line 15: expected '=' but found ':'
```

**Invalid Values:**
```
Error: clipboard.max_size must be between 1024 and 52428800
Error: hotkeys.show_history contains invalid key combination
Error: auth.ssh_key file does not exist: ~/.ssh/nonexistent
```

**Network Issues:**
```
Error: listen_addr port 8484 is already in use
Warning: advertise_name contains invalid characters
```

## 🔄 Configuration Profiles

### Creating Profiles

```bash
# Create work profile
cp ~/.config/clipsync/config.toml ~/.config/clipsync/work.toml

# Create home profile  
cp ~/.config/clipsync/config.toml ~/.config/clipsync/home.toml

# Create travel profile (minimal features)
clipsync config init --template minimal > ~/.config/clipsync/travel.toml
```

### Using Profiles

```bash
# Start with specific profile
clipsync --config ~/.config/clipsync/work.toml start

# Switch profiles
clipsync stop
clipsync --config ~/.config/clipsync/home.toml start
```

### Profile Switching Script

```bash
#!/bin/bash
# ~/.local/bin/clipsync-profile

PROFILE_DIR="$HOME/.config/clipsync"

case "$1" in
    work)
        clipsync stop
        clipsync --config "$PROFILE_DIR/work.toml" start
        ;;
    home)
        clipsync stop  
        clipsync --config "$PROFILE_DIR/home.toml" start
        ;;
    travel)
        clipsync stop
        clipsync --config "$PROFILE_DIR/travel.toml" start
        ;;
    *)
        echo "Usage: clipsync-profile {work|home|travel}"
        ;;
esac
```

## 🚨 Configuration Troubleshooting

### Common Issues

**Config File Not Found:**
```bash
# Check if config file exists
ls ~/.config/clipsync/config.toml

# Create default config
clipsync config init
```

**Permission Errors:**
```bash
# Fix config directory permissions
chmod 700 ~/.config/clipsync
chmod 600 ~/.config/clipsync/config.toml
```

**SSH Key Issues:**
```bash
# Verify SSH key exists and has correct permissions
ls -la ~/.ssh/id_ed25519*
chmod 600 ~/.ssh/id_ed25519
chmod 644 ~/.ssh/id_ed25519.pub
```

**Port Conflicts:**
```bash
# Check if port is in use
lsof -i :8484

# Use different port in config
listen_addr = ":8485"
```

### Debug Configuration

```bash
# Show effective configuration
clipsync config show

# Test configuration
clipsync start --foreground --config ~/.config/clipsync/config.toml

# Enable debug logging
export RUST_LOG=debug
clipsync start --foreground
```

---

**Related Documentation:**
- [User Guide](USER_GUIDE.md) - Learn how to use ClipSync effectively
- [Installation Guide](INSTALL.md) - Installation instructions
- [Troubleshooting](TROUBLESHOOTING.md) - Common issues and solutions
- [Security Guide](SECURITY.md) - Security best practices