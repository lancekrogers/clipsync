# 🤝 Contributing to ClipSync

Thank you for your interest in contributing to ClipSync! This guide will help you get started with development, understand our processes, and make meaningful contributions.

## 📋 Table of Contents

1. [Getting Started](#-getting-started)
2. [Development Setup](#-development-setup)
3. [Project Architecture](#-project-architecture)
4. [Coding Standards](#-coding-standards)
5. [Testing Guidelines](#-testing-guidelines)
6. [Pull Request Process](#-pull-request-process)
7. [Issue Guidelines](#-issue-guidelines)
8. [Security Guidelines](#-security-guidelines)
9. [Release Process](#-release-process)

## 🚀 Getting Started

### Ways to Contribute

- 🐛 **Bug Reports**: Found an issue? Let us know!
- 💡 **Feature Requests**: Have an idea? We'd love to hear it!
- 📝 **Documentation**: Help improve our docs
- 🔧 **Code**: Fix bugs or implement new features
- 🧪 **Testing**: Help us test on different platforms
- 🌍 **Translations**: Help make ClipSync accessible worldwide

### Before You Start

1. **Check existing issues** to avoid duplicating work
2. **Read our [Code of Conduct](CODE_OF_CONDUCT.md)**
3. **Join our [Discord/Matrix]** for questions and discussion
4. **Star the repository** if you find it useful!

## 🛠️ Development Setup

### Prerequisites

**Required:**
- **Rust**: 1.75.0 or later
- **Git**: For version control
- **Platform-specific dependencies** (see below)

**macOS Dependencies:**
```bash
# Install Xcode Command Line Tools
xcode-select --install

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

**Linux Dependencies:**
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

### Clone and Build

```bash
# Clone the repository
git clone https://github.com/yourusername/clipsync.git
cd clipsync

# Install development tools
cargo install cargo-watch cargo-expand cargo-audit

# Build the project
cargo build

# Run tests
cargo test

# Start development server with auto-reload
cargo watch -x "run -- start --foreground"
```

### Development Tools

**Recommended VS Code Extensions:**
- `rust-analyzer` - Rust language support
- `CodeLLDB` - Debugging support
- `crates` - Crate dependency management
- `Better TOML` - TOML syntax highlighting

**Recommended Tools:**
```bash
# Code formatting and linting
rustup component add rustfmt clippy

# Security auditing
cargo install cargo-audit

# Code coverage
cargo install cargo-tarpaulin

# Documentation generation
cargo install cargo-doc
```

## 🏗️ Project Architecture

### Directory Structure

```
clipsync/
├── src/
│   ├── auth/           # Authentication and SSH key management
│   ├── clipboard/      # Platform-specific clipboard access
│   ├── cli/            # Command-line interface
│   ├── config/         # Configuration management
│   ├── discovery/      # Peer discovery (mDNS/DNS-SD)
│   ├── history/        # Clipboard history database
│   ├── hotkey/         # Global hotkey system
│   ├── transport/      # Network transport layer
│   ├── service/        # Background service management
│   └── main.rs         # Application entry point
├── tests/              # Integration tests
├── benches/            # Performance benchmarks
├── docs/               # Documentation
├── scripts/            # Build and deployment scripts
├── .github/            # GitHub workflows and templates
└── assets/             # Icons, logos, and resources
```

### Core Modules

#### 1. Authentication (`src/auth/`)
- SSH key-based peer authentication
- Public key management and verification
- Token generation and validation

**Key Files:**
- `mod.rs` - Public API and trait definitions
- `ssh.rs` - SSH key handling and crypto operations
- `store.rs` - Authorized keys storage

#### 2. Clipboard (`src/clipboard/`)
- Platform-specific clipboard access
- Content type detection and conversion
- Clipboard monitoring and change detection

**Key Files:**
- `mod.rs` - Cross-platform clipboard abstraction
- `macos.rs` - NSPasteboard integration
- `linux.rs` - X11/Wayland clipboard handling

#### 3. Transport (`src/transport/`)
- WebSocket-based network communication
- Message protocol and serialization
- Connection management and reconnection

**Key Files:**
- `mod.rs` - Transport traits and error types
- `websocket.rs` - WebSocket implementation
- `protocol.rs` - Message protocol definitions
- `stream.rs` - Large payload streaming
- `reconnect.rs` - Automatic reconnection logic

#### 4. Discovery (`src/discovery/`)
- mDNS/DNS-SD service discovery
- Peer information management
- Network change detection

#### 5. History (`src/history/`)
- SQLite database with encryption
- Clipboard history management
- Search and retrieval operations

### Module Dependencies

```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│     CLI     │────│   Config    │────│   Service   │
└─────────────┘    └─────────────┘    └─────────────┘
        │                                     │
        ▼                                     ▼
┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│  Clipboard  │────│   History   │    │  Discovery  │
└─────────────┘    └─────────────┘    └─────────────┘
        │                                     │
        ▼                                     ▼
┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│   Hotkey    │    │    Auth     │────│  Transport  │
└─────────────┘    └─────────────┘    └─────────────┘
```

## 📏 Coding Standards

### Rust Code Style

We follow the official Rust style guide with some additional conventions:

**Formatting:**
```bash
# Auto-format code
cargo fmt

# Check formatting
cargo fmt -- --check
```

**Linting:**
```bash
# Run Clippy lints
cargo clippy -- -D warnings

# Run Clippy with all features
cargo clippy --all-features -- -D warnings
```

### Code Organization

**1. Module Structure:**
```rust
//! Module documentation goes here
//!
//! Longer description of what this module does.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

// Re-exports
pub use self::error::*;
pub use self::types::*;

// Public types and traits
pub trait Example {
    fn method(&self) -> Result<()>;
}

// Public structs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicStruct {
    field: String,
}

// Implementation blocks
impl PublicStruct {
    pub fn new(field: String) -> Self {
        Self { field }
    }
}

// Private items at the bottom
mod error;
mod types;

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_example() {
        // Test implementation
    }
}
```

**2. Error Handling:**
```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("Clipboard access denied")]
    AccessDenied,
    
    #[error("Unsupported content type: {mime_type}")]
    UnsupportedContentType { mime_type: String },
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ClipboardError>;
```

**3. Async Code:**
```rust
use tokio::time::{sleep, Duration};
use tracing::{info, error, instrument};

#[instrument(skip(self))]
pub async fn sync_clipboard(&mut self) -> Result<()> {
    info!("Starting clipboard sync");
    
    let content = self.clipboard.get_content().await?;
    
    match self.transport.send(content).await {
        Ok(_) => {
            info!("Clipboard synced successfully");
            Ok(())
        }
        Err(e) => {
            error!("Failed to sync clipboard: {}", e);
            Err(e)
        }
    }
}
```

### Documentation Standards

**1. Module Documentation:**
```rust
//! # Clipboard Module
//!
//! This module provides cross-platform clipboard access and monitoring.
//!
//! ## Example
//!
//! ```rust
//! use clipsync::clipboard::Clipboard;
//!
//! let mut clipboard = Clipboard::new()?;
//! clipboard.set_text("Hello, world!")?;
//! let content = clipboard.get_text()?;
//! ```

pub struct Clipboard {
    // implementation
}
```

**2. Function Documentation:**
```rust
/// Sets the clipboard content to the specified text.
///
/// # Arguments
///
/// * `text` - The text to set in the clipboard
///
/// # Errors
///
/// Returns an error if the clipboard is inaccessible or if the text
/// is too large for the clipboard.
///
/// # Example
///
/// ```rust
/// let mut clipboard = Clipboard::new()?;
/// clipboard.set_text("Hello, world!")?;
/// ```
pub fn set_text(&mut self, text: &str) -> Result<()> {
    // implementation
}
```

### Naming Conventions

- **Types**: `PascalCase` (e.g., `ClipboardManager`)
- **Functions/Variables**: `snake_case` (e.g., `get_clipboard_content`)
- **Constants**: `SCREAMING_SNAKE_CASE` (e.g., `MAX_CLIPBOARD_SIZE`)
- **Modules**: `snake_case` (e.g., `clipboard_manager`)

## 🧪 Testing Guidelines

### Test Organization

```
tests/
├── integration/        # Integration tests
│   ├── auth_test.rs
│   ├── clipboard_test.rs
│   └── transport_test.rs
├── fixtures/           # Test data and fixtures
├── common/             # Shared test utilities
└── benches/            # Performance benchmarks
```

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio_test;
    
    #[test]
    fn test_clipboard_set_get() {
        let mut clipboard = Clipboard::new().unwrap();
        let test_text = "Hello, test!";
        
        clipboard.set_text(test_text).unwrap();
        let result = clipboard.get_text().unwrap();
        
        assert_eq!(result, test_text);
    }
    
    #[tokio::test]
    async fn test_async_sync() {
        let mut sync = ClipboardSync::new().await.unwrap();
        
        let result = sync.sync_clipboard().await;
        assert!(result.is_ok());
    }
}
```

### Integration Tests

```rust
// tests/integration/clipboard_test.rs
use clipsync::prelude::*;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::test]
async fn test_full_sync_workflow() {
    // Set up test environment
    let config1 = test_config("device1");
    let config2 = test_config("device2");
    
    let mut sync1 = ClipSync::new(config1).await.unwrap();
    let mut sync2 = ClipSync::new(config2).await.unwrap();
    
    // Start both services
    sync1.start().await.unwrap();
    sync2.start().await.unwrap();
    
    // Wait for connection
    sleep(Duration::from_secs(1)).await;
    
    // Test clipboard sync
    sync1.set_clipboard("test content").await.unwrap();
    sleep(Duration::from_millis(500)).await;
    
    let content = sync2.get_clipboard().await.unwrap();
    assert_eq!(content, "test content");
    
    // Cleanup
    sync1.stop().await.unwrap();
    sync2.stop().await.unwrap();
}
```

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_clipboard_set_get

# Run integration tests only
cargo test --test integration

# Run with output
cargo test -- --nocapture

# Run tests with coverage
cargo tarpaulin --out Html
```

### Benchmarks

```rust
// benches/clipboard_bench.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use clipsync::clipboard::Clipboard;

fn clipboard_benchmark(c: &mut Criterion) {
    let mut clipboard = Clipboard::new().unwrap();
    
    c.bench_function("clipboard_set_text", |b| {
        b.iter(|| {
            clipboard.set_text(black_box("benchmark text")).unwrap();
        })
    });
    
    c.bench_function("clipboard_get_text", |b| {
        clipboard.set_text("benchmark text").unwrap();
        b.iter(|| {
            black_box(clipboard.get_text().unwrap());
        })
    });
}

criterion_group!(benches, clipboard_benchmark);
criterion_main!(benches);
```

## 🔄 Pull Request Process

### Before Submitting

1. **Create an issue** for bugs or feature requests (unless it's a small fix)
2. **Fork the repository** and create a feature branch
3. **Write tests** for your changes
4. **Update documentation** if needed
5. **Run the test suite** and ensure everything passes
6. **Run code formatting** and linting

### Branch Naming

- `feature/add-clipboard-history` - New features
- `bugfix/fix-connection-timeout` - Bug fixes
- `docs/update-readme` - Documentation updates
- `refactor/cleanup-auth-module` - Code refactoring

### Commit Messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
type(scope): description

[optional body]

[optional footer]
```

**Examples:**
```
feat(clipboard): add support for image synchronization

- Implement PNG and JPEG image handling
- Add image compression for network transfer
- Update clipboard protocol to support binary data

Closes #123

fix(transport): resolve connection timeout issue

The WebSocket connection was timing out due to incorrect
keepalive settings. This commit adjusts the timeout values
and adds proper error handling.

Fixes #456

docs(readme): update installation instructions

- Add Homebrew installation steps
- Update Linux package manager instructions
- Fix broken links to documentation

chore(deps): update tokio to 1.35.0
```

### Pull Request Template

```markdown
## Description
Brief description of changes

## Type of Change
- [ ] Bug fix
- [ ] New feature
- [ ] Breaking change
- [ ] Documentation update

## Testing
- [ ] Unit tests pass
- [ ] Integration tests pass
- [ ] Manual testing completed

## Checklist
- [ ] Code follows style guidelines
- [ ] Self-review completed
- [ ] Documentation updated
- [ ] Tests added for changes
```

### Review Process

1. **Automated checks** must pass (CI, linting, tests)
2. **At least one maintainer** must approve
3. **All conversations** must be resolved
4. **Squash and merge** for clean history

## 📝 Issue Guidelines

### Bug Reports

**Template:**
```markdown
## Bug Description
Clear description of the bug

## Steps to Reproduce
1. Start ClipSync
2. Copy text on device A
3. Observe behavior on device B

## Expected Behavior
Text should appear on device B

## Actual Behavior
Text does not sync

## Environment
- OS: macOS 14.0
- ClipSync version: 1.0.0
- Installation method: Homebrew

## Additional Context
- Error logs
- Screenshots
- Configuration file (redacted)
```

### Feature Requests

**Template:**
```markdown
## Feature Description
Clear description of the proposed feature

## Use Case
Why this feature would be useful

## Proposed Solution
How you think it should work

## Alternatives Considered
Other approaches you've considered

## Additional Context
Any other relevant information
```

## 🔐 Security Guidelines

### Security-Sensitive Changes

For security-related contributions:

1. **Create a private security advisory** first
2. **Discuss with maintainers** before implementation
3. **Follow security review process**
4. **Coordinate disclosure timeline**

### Secure Coding Practices

**1. Input Validation:**
```rust
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct ClipboardContent {
    #[serde(deserialize_with = "validate_content_size")]
    content: Vec<u8>,
}

fn validate_content_size<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let content = Vec::<u8>::deserialize(deserializer)?;
    if content.len() > MAX_CONTENT_SIZE {
        return Err(serde::de::Error::custom("Content too large"));
    }
    Ok(content)
}
```

**2. Secure Memory Handling:**
```rust
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(ZeroizeOnDrop)]
pub struct PrivateKey {
    key_data: Vec<u8>,
}

impl Drop for PrivateKey {
    fn drop(&mut self) {
        self.key_data.zeroize();
    }
}
```

**3. Cryptographic Operations:**
```rust
use ring::rand::{SecureRandom, SystemRandom};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};

pub fn encrypt_data(data: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    let rng = SystemRandom::new();
    let unbound_key = UnboundKey::new(&AES_256_GCM, key)?;
    let key = LessSafeKey::new(unbound_key);
    
    let mut nonce_bytes = [0u8; 12];
    rng.fill(&mut nonce_bytes)?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    
    let mut ciphertext = data.to_vec();
    key.seal_in_place_append_tag(nonce, Aad::empty(), &mut ciphertext)?;
    
    Ok([&nonce_bytes[..], &ciphertext].concat())
}
```

## 🚀 Release Process

### Version Numbering

We follow [Semantic Versioning](https://semver.org/):
- **MAJOR**: Incompatible API changes
- **MINOR**: New functionality (backward compatible)
- **PATCH**: Bug fixes (backward compatible)

### Release Checklist

**Pre-release:**
- [ ] Update version numbers
- [ ] Update CHANGELOG.md
- [ ] Run full test suite
- [ ] Update documentation
- [ ] Security audit
- [ ] Performance benchmarks

**Release:**
- [ ] Create release tag
- [ ] Build release artifacts
- [ ] Upload to package repositories
- [ ] Update Homebrew formula
- [ ] Announce release

### Changelog Format

```markdown
# Changelog

## [1.2.0] - 2024-01-15

### Added
- Image synchronization support
- Configuration profiles
- Interactive history picker

### Changed
- Improved connection stability
- Updated dependencies

### Fixed
- Memory leak in clipboard monitoring
- Race condition in peer discovery

### Security
- Enhanced key validation
- Improved encryption key derivation

### Deprecated
- Old configuration format (will be removed in 2.0.0)

### Removed
- Legacy protocol support
```

## 🎯 Getting Help

### Communication Channels

- **GitHub Issues**: Bug reports and feature requests
- **GitHub Discussions**: General questions and ideas
- **Discord**: Real-time chat and support
- **Email**: security@clipsync.dev (security issues only)

### Development Resources

- **API Documentation**: `cargo doc --open`
- **Architecture Decision Records**: `docs/adr/`
- **Development Blog**: Technical deep-dives and updates
- **Community Wiki**: Shared knowledge and tutorials

## 📄 License

By contributing to ClipSync, you agree that your contributions will be licensed under both the MIT License and Apache License 2.0, matching the project's dual license.

---

**Thank you for contributing to ClipSync!** 🎉

Your contributions help make secure clipboard synchronization accessible to everyone. Whether you're fixing a typo or implementing a major feature, every contribution matters.

*Happy coding!* 🦀