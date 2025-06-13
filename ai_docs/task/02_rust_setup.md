# Task 02: Rust Project Setup

## Objective
Initialize the Rust project with Cargo and set up core dependencies.

## Steps

1. **Initialize Cargo project**
   ```bash
   cargo init --name clipsync
   ```

2. **Update Cargo.toml with project metadata**
   ```toml
   [package]
   name = "clipsync"
   version = "0.1.0"
   edition = "2021"
   authors = ["Your Name <email@example.com>"]
   description = "Cross-platform clipboard synchronization with history"
   license = "MIT OR Apache-2.0"
   repository = "https://github.com/yourusername/clipsync"
   keywords = ["clipboard", "sync", "cross-platform"]
   categories = ["command-line-utilities"]
   
   [profile.release]
   opt-level = 3
   lto = true
   codegen-units = 1
   strip = true
   ```

3. **Add core dependencies to Cargo.toml**
   ```toml
   [dependencies]
   # Async runtime
   tokio = { version = "1.35", features = ["full"] }
   
   # Clipboard access
   arboard = "3.3"
   
   # Networking
   tokio-tungstenite = "0.21"
   ssh2 = "0.9"
   
   # Database
   rusqlite = { version = "0.30", features = ["bundled-sqlcipher"] }
   
   # Encryption & Security
   aes-gcm = "0.10"
   argon2 = "0.5"
   keyring = "2.3"
   zeroize = "1.7"
   rand = "0.8"
   sha2 = "0.10"
   
   # Serialization
   serde = { version = "1.0", features = ["derive"] }
   serde_json = "1.0"
   bincode = "1.3"
   toml = "0.8"
   
   # Service discovery
   mdns-sd = "0.10"
   
   # CLI
   clap = { version = "4.4", features = ["derive"] }
   
   # Logging
   tracing = "0.1"
   tracing-subscriber = "0.3"
   
   # Error handling
   thiserror = "1.0"
   anyhow = "1.0"
   
   # Compression
   zstd = "0.13"
   
   # Utils
   uuid = { version = "1.6", features = ["v4", "serde"] }
   chrono = "0.4"
   directories = "5.0"
   
   [dev-dependencies]
   criterion = "0.5"
   tempfile = "3.8"
   mockall = "0.12"
   
   [target.'cfg(target_os = "macos")'.dependencies]
   cocoa = "0.25"
   objc = "0.2"
   
   [target.'cfg(target_os = "linux")'.dependencies]
   x11-clipboard = "0.9"
   wayland-client = "0.31"
   ```

4. **Create rust-toolchain.toml**
   ```toml
   [toolchain]
   channel = "stable"
   components = ["rustfmt", "clippy"]
   ```

5. **Create .cargo/config.toml**
   ```toml
   [build]
   target-dir = "target"
   
   [target.x86_64-unknown-linux-gnu]
   linker = "clang"
   
   [target.x86_64-apple-darwin]
   rustflags = ["-C", "link-arg=-framework", "AppKit"]
   
   [target.aarch64-apple-darwin]
   rustflags = ["-C", "link-arg=-framework", "AppKit"]
   ```

6. **Create initial src/main.rs**
   ```rust
   fn main() {
       println!("ClipSync v{}", env!("CARGO_PKG_VERSION"));
   }
   ```

7. **Create src/lib.rs**
   ```rust
   pub mod clipboard;
   pub mod transport;
   pub mod history;
   pub mod config;
   pub mod discovery;
   ```

## Success Criteria
- Cargo project initialized
- All dependencies added
- Project compiles with `cargo build`
- Toolchain configured for cross-platform builds