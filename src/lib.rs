//! # ClipSync
//!
//! Cross-platform clipboard synchronization service for macOS and Linux.
//!
//! ClipSync provides secure, peer-to-peer clipboard synchronization between
//! devices with support for text, images, and files.
// Temporarily allow all warnings to focus on compilation errors
#![allow(warnings)]
#![allow(clippy::all)]
#![allow(clippy::pedantic)]
#![allow(clippy::nursery)]
#![allow(clippy::cargo)]
#![allow(clippy::module_name_repetitions)]

//! ClipSync - Cross-platform clipboard synchronization library
//!
//! This library provides the core functionality for synchronizing clipboard
//! content between macOS and Linux systems.

pub mod adapters;
pub mod auth;
pub mod cli;
pub mod clipboard;
pub mod config;
#[cfg(target_os = "linux")]
pub mod daemon;
pub mod discovery;
pub mod history;
// pub mod hotkey; // Removed - we work with system clipboard
pub mod progress;
pub mod setup;
pub mod sync;
pub mod transport;

pub use config::Config;

/// Result type alias for ClipSync operations
pub type Result<T> = std::result::Result<T, Error>;

/// Error types for ClipSync operations
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Authentication error
    #[error("Authentication error: {0}")]
    Auth(#[from] auth::AuthError),

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(#[from] config::ConfigError),

    /// Clipboard operation error
    #[error("Clipboard error: {0}")]
    Clipboard(#[from] clipboard::ClipboardError),

    /// Transport error
    #[error("Transport error: {0}")]
    Transport(#[from] transport::TransportError),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Generic error
    #[error("{0}")]
    Other(String),
}

/// Library version information
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Maximum clipboard payload size (5MB default)
pub const MAX_PAYLOAD_SIZE: usize = 5 * 1024 * 1024;
