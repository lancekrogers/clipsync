# ClipSync Project Structure

## Top-Level Files

- `Cargo.toml`, `Cargo.lock` - Rust project configuration
- `rust-toolchain.toml` - Rust toolchain specification
- `Makefile` - Build and installation commands
- `justfile` - Alternative task runner
- `README.md` - Main project documentation
- `CHANGELOG.md` - Version history
- `CONTRIBUTING.md` - Contribution guidelines
- `INSTALL.md` - Installation instructions
- `LICENSE-APACHE`, `LICENSE-MIT` - Dual licensing
- `install_user.sh` - User installation script (no sudo)
- `uninstall_user.sh` - User uninstallation script

## Directory Structure

```
clipsync/
├── src/                    # Source code
│   ├── auth/              # Authentication (SSH keys)
│   ├── clipboard/         # Platform-specific clipboard
│   ├── cli/               # Command-line interface
│   ├── config/            # Configuration management
│   ├── discovery/         # Service discovery (mDNS)
│   ├── history/           # Clipboard history & encryption
│   ├── hotkey/            # Global hotkey support
│   ├── sync/              # Synchronization engine
│   └── transport/         # Network transport (WebSocket)
├── tests/                  # Integration tests
├── examples/               # Example usage code
├── scripts/                # Build and utility scripts
│   ├── test/              # Test scripts
│   └── package/           # Packaging scripts
├── docs/                   # Documentation
├── pkg/                    # Platform-specific packaging
│   ├── aur/               # Arch Linux AUR
│   ├── debian/            # Debian/Ubuntu
│   ├── homebrew/          # macOS Homebrew
│   ├── macos/             # macOS installer
│   └── rpm/               # RedHat/Fedora
├── homebrew/               # Homebrew formula
└── ai_docs/                # Development documentation
    └── archive/           # Historical development notes

## Build Output

- `target/` - Rust build artifacts (gitignored)
- `dist/` - Distribution packages (gitignored)

## Configuration

User configuration is stored in:
- macOS/Linux: `~/.config/clipsync/`
- Config file: `config.toml`
- SSH keys: `~/.config/clipsync/keys/`
- History database: `~/.config/clipsync/history.db`