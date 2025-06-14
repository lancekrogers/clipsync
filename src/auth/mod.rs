//! SSH-based authentication module for ClipSync
//!
//! This module provides SSH key-based authentication for secure peer-to-peer
//! connections in ClipSync.

use std::path::PathBuf;
use thiserror::Error;

pub mod authorized;
pub mod keys;
pub mod ssh;
pub mod trust;

pub use authorized::{AuthorizedKey, AuthorizedKeys};
pub use keys::{KeyPair, KeyType, PublicKey};
pub use ssh::{AuthToken, PeerId, SshAuthenticator};
pub use trust::{TrustDecision, TrustManager, TrustStatus};

/// Authentication errors
#[derive(Debug, Error)]
pub enum AuthError {
    /// SSH key error
    #[error("SSH key error: {0}")]
    KeyError(String),

    /// Authentication failed
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Key not found
    #[error("Key not found: {0}")]
    KeyNotFound(PathBuf),

    /// Invalid key format
    #[error("Invalid key format: {0}")]
    InvalidKeyFormat(String),

    /// Unauthorized peer
    #[error("Unauthorized peer: {0}")]
    UnauthorizedPeer(String),

    /// Crypto error
    #[error("Cryptography error: {0}")]
    CryptoError(String),
}

/// Authentication trait for peer authentication
#[async_trait::async_trait]
pub trait Authenticator: Send + Sync {
    /// Authenticate a peer using their public key
    async fn authenticate_peer(&self, peer_key: &PublicKey) -> Result<AuthToken, AuthError>;

    /// Verify an authentication token
    async fn verify_token(&self, token: &AuthToken) -> Result<PeerId, AuthError>;

    /// Get the local public key
    async fn get_public_key(&self) -> Result<PublicKey, AuthError>;

    /// Check if a peer is authorized
    async fn is_authorized(&self, peer_key: &PublicKey) -> Result<bool, AuthError>;
}

/// Authentication configuration
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Path to SSH private key
    pub private_key_path: PathBuf,

    /// Path to authorized keys file
    pub authorized_keys_path: PathBuf,

    /// Whether to generate a new key if none exists
    pub generate_if_missing: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            private_key_path: dirs::home_dir()
                .unwrap_or_default()
                .join(".ssh")
                .join("id_ed25519"),
            authorized_keys_path: dirs::home_dir()
                .unwrap_or_default()
                .join(".config")
                .join("clipsync")
                .join("authorized_keys"),
            generate_if_missing: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_auth_config() {
        let config = AuthConfig::default();
        assert!(config
            .private_key_path
            .to_string_lossy()
            .contains("id_ed25519"));
        assert!(config
            .authorized_keys_path
            .to_string_lossy()
            .contains("authorized_keys"));
        assert!(config.generate_if_missing);
    }
}
