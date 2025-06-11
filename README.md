# ClipSync

ClipSync is a high-performance, cross-platform clipboard synchronization service written in Rust. It enables real-time clipboard sharing between macOS and Linux systems with end-to-end encryption and SSH key authentication.

## Features

- **Cross-Platform Support**: Works seamlessly between macOS and Linux (X11/Wayland)
- **Real-Time Sync**: Instant clipboard synchronization across devices
- **Secure**: SSH key-based authentication and end-to-end encryption
- **Rich Content Support**: Handles text, RTF, and images up to 5MB
- **History**: Maintains an encrypted history of the last 20 clipboard items
- **Performance**: Built in Rust for maximum efficiency and safety
- **Service Discovery**: Automatic peer discovery using DNS-SD/mDNS

## Installation

### From Binary (Recommended)

Download the latest release for your platform from the [releases page](https://github.com/yourusername/clipsync/releases).

### From Source

```bash
# Clone the repository
git clone https://github.com/yourusername/clipsync.git
cd clipsync

# Build the project
cargo build --release

# Install
cargo install --path .
```

## Usage

### Starting the Service

```bash
# Start ClipSync daemon
clipsync start

# Start with custom config
clipsync --config ~/.config/clipsync/config.toml start
```

### CLI Commands

```bash
# Show current clipboard content
clipsync show

# Show clipboard history
clipsync history

# Copy specific item from history
clipsync copy <index>

# List connected peers
clipsync peers

# Stop the service
clipsync stop
```

## Configuration

ClipSync looks for configuration in the following locations:
- `~/.config/clipsync/config.toml` (Linux)
- `~/Library/Application Support/ClipSync/config.toml` (macOS)

Example configuration:

```toml
[server]
port = 8080
host = "0.0.0.0"

[auth]
ssh_key_path = "~/.ssh/id_ed25519"

[history]
max_items = 20
database_path = "~/.local/share/clipsync/history.db"

[sync]
max_payload_size = 5242880  # 5MB
compression = true
```

## Development

### Prerequisites

- Rust 1.70 or higher
- Cargo
- Platform-specific dependencies:
  - **macOS**: Xcode Command Line Tools
  - **Linux**: libx11-dev, libxcb-dev, libssl-dev

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Run with verbose logging
RUST_LOG=debug cargo run
```

### Cross-Compilation

```bash
# Build for all supported platforms
make build-all

# Build for specific target
cargo build --target x86_64-apple-darwin --release
```

## Architecture

ClipSync uses a modular architecture:

- **Clipboard Module**: Platform-specific clipboard access
- **Transport Layer**: WebSocket over TLS with SSH authentication
- **History Database**: SQLite with encryption for clipboard history
- **Service Discovery**: mDNS/DNS-SD for automatic peer discovery
- **CLI Interface**: Command-line interface for user interaction

## Contributing

Contributions are welcome! Please read our [Contributing Guidelines](CONTRIBUTING.md) before submitting PRs.

## License

ClipSync is dual-licensed under:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

You may choose either license for your use.

## Security

ClipSync takes security seriously:

- All network communication is encrypted using TLS
- SSH key-based authentication prevents unauthorized access
- Clipboard history is encrypted at rest
- No data is sent to third-party servers

For security issues, please email security@clipsync.dev

## Acknowledgments

Built with excellent Rust crates including:
- tokio for async runtime
- serde for serialization
- sqlcipher for encrypted database
- and many more...

## Status

This project is under active development. See the [roadmap](ROADMAP.md) for planned features.