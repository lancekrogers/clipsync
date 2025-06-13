//! SSH authentication implementation

use crate::auth::{AuthConfig, AuthError, Authenticator, KeyPair, PublicKey};
use async_trait::async_trait;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

/// Authentication token for verified peers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthToken {
    /// Unique token ID
    pub token_id: String,
    /// Peer's public key fingerprint
    pub peer_fingerprint: String,
    /// Token creation timestamp
    pub created_at: u64,
    /// Token expiration timestamp
    pub expires_at: u64,
    /// Digital signature
    pub signature: Vec<u8>,
}

impl AuthToken {
    /// Check if the token is expired
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now > self.expires_at
    }

    /// Convert token to string representation
    pub fn to_string(&self) -> String {
        // Create a simple string representation of the token
        format!(
            "{}:{}:{}",
            self.token_id, self.peer_fingerprint, self.created_at
        )
    }
}

impl std::fmt::Display for AuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            Some(name) => write!(f, "{} ({})", name, &self.fingerprint[..8]),
            None => write!(f, "{}", &self.fingerprint[..8]),
        }
    }
}

/// Peer identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerId {
    /// Public key fingerprint
    pub fingerprint: String,
    /// Optional peer name
    pub name: Option<String>,
}

/// SSH-based authenticator
pub struct SshAuthenticator {
    /// Local key pair
    key_pair: Arc<RwLock<Option<KeyPair>>>,
    /// Configuration
    config: AuthConfig,
    /// Authorized keys
    authorized_keys: Arc<RwLock<crate::auth::AuthorizedKeys>>,
    /// Active tokens
    active_tokens: Arc<RwLock<std::collections::HashMap<String, AuthToken>>>,
    /// Random number generator
    rng: SystemRandom,
}

impl SshAuthenticator {
    /// Create a new SSH authenticator
    pub async fn new(config: AuthConfig) -> Result<Self, AuthError> {
        // Load or generate key pair
        let key_pair = if config.private_key_path.exists() {
            Some(crate::auth::keys::KeyPair::load_from_file(&config.private_key_path).await?)
        } else if config.generate_if_missing {
            let key_pair = crate::auth::keys::KeyPair::generate(crate::auth::KeyType::Ed25519)?;
            key_pair.save_to_file(&config.private_key_path).await?;
            Some(key_pair)
        } else {
            None
        };

        // Load authorized keys
        let authorized_keys = if config.authorized_keys_path.exists() {
            crate::auth::AuthorizedKeys::load_from_file(&config.authorized_keys_path).await?
        } else {
            crate::auth::AuthorizedKeys::new()
        };

        Ok(Self {
            key_pair: Arc::new(RwLock::new(key_pair)),
            config,
            authorized_keys: Arc::new(RwLock::new(authorized_keys)),
            active_tokens: Arc::new(RwLock::new(std::collections::HashMap::new())),
            rng: SystemRandom::new(),
        })
    }

    /// Generate a new authentication token
    async fn generate_token(&self, peer_key: &PublicKey) -> Result<AuthToken, AuthError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Generate random token ID
        let mut token_bytes = [0u8; 32];
        self.rng
            .fill(&mut token_bytes)
            .map_err(|e| AuthError::CryptoError(e.to_string()))?;
        let token_id = hex::encode(token_bytes);

        let token = AuthToken {
            token_id: token_id.clone(),
            peer_fingerprint: peer_key.fingerprint(),
            created_at: now,
            expires_at: now + 3600, // 1 hour expiration
            signature: Vec::new(),  // Will be filled in by sign_token
        };

        // Sign the token
        let signed_token = self.sign_token(token).await?;

        // Store the token
        let mut tokens = self.active_tokens.write().await;
        tokens.insert(token_id, signed_token.clone());

        Ok(signed_token)
    }

    /// Sign a token with the local private key
    async fn sign_token(&self, mut token: AuthToken) -> Result<AuthToken, AuthError> {
        let key_pair = self.key_pair.read().await;
        let key_pair = key_pair
            .as_ref()
            .ok_or_else(|| AuthError::KeyError("No local key pair available".to_string()))?;

        // Create data to sign
        let data_to_sign = format!(
            "{}:{}:{}:{}",
            token.token_id, token.peer_fingerprint, token.created_at, token.expires_at
        );

        // Sign the data
        token.signature = key_pair.sign(data_to_sign.as_bytes())?;

        Ok(token)
    }

    /// Verify a token signature using our local public key
    async fn verify_token_signature(&self, token: &AuthToken) -> Result<bool, AuthError> {
        let local_public_key = self.get_public_key().await?;

        let data_to_verify = format!(
            "{}:{}:{}:{}",
            token.token_id, token.peer_fingerprint, token.created_at, token.expires_at
        );

        local_public_key.verify(data_to_verify.as_bytes(), &token.signature)
    }

    /// Clean up expired tokens
    async fn cleanup_expired_tokens(&self) {
        let mut tokens = self.active_tokens.write().await;
        tokens.retain(|_, token| !token.is_expired());
    }
}

#[async_trait]
impl Authenticator for SshAuthenticator {
    async fn authenticate_peer(&self, peer_key: &PublicKey) -> Result<AuthToken, AuthError> {
        // Clean up expired tokens
        self.cleanup_expired_tokens().await;

        // Check if peer is authorized
        if !self.is_authorized(peer_key).await? {
            return Err(AuthError::UnauthorizedPeer(peer_key.fingerprint()));
        }

        // Generate authentication token
        self.generate_token(peer_key).await
    }

    async fn verify_token(&self, token: &AuthToken) -> Result<PeerId, AuthError> {
        // Check if token exists and is not expired
        let tokens = self.active_tokens.read().await;
        let stored_token = tokens
            .get(&token.token_id)
            .ok_or_else(|| AuthError::AuthenticationFailed("Token not found".to_string()))?;

        if stored_token.is_expired() {
            return Err(AuthError::AuthenticationFailed("Token expired".to_string()));
        }

        // Verify token matches
        if stored_token.peer_fingerprint != token.peer_fingerprint
            || stored_token.created_at != token.created_at
            || stored_token.expires_at != token.expires_at
        {
            return Err(AuthError::AuthenticationFailed(
                "Token mismatch".to_string(),
            ));
        }

        // Get peer's authorized key for name lookup
        let authorized_keys = self.authorized_keys.read().await;
        let peer_key = authorized_keys
            .get_key_by_fingerprint(&token.peer_fingerprint)
            .ok_or_else(|| AuthError::AuthenticationFailed("Peer key not found".to_string()))?;

        // Verify signature (tokens are signed by us, not the peer)
        if !self.verify_token_signature(token).await? {
            return Err(AuthError::AuthenticationFailed(
                "Invalid signature".to_string(),
            ));
        }

        Ok(PeerId {
            fingerprint: token.peer_fingerprint.clone(),
            name: peer_key.comment.clone(),
        })
    }

    async fn get_public_key(&self) -> Result<PublicKey, AuthError> {
        let key_pair = self.key_pair.read().await;
        let key_pair = key_pair
            .as_ref()
            .ok_or_else(|| AuthError::KeyError("No local key pair available".to_string()))?;

        Ok(key_pair.public_key())
    }

    async fn is_authorized(&self, peer_key: &PublicKey) -> Result<bool, AuthError> {
        let authorized_keys = self.authorized_keys.read().await;
        Ok(authorized_keys.is_authorized(peer_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_authenticator() -> (SshAuthenticator, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = AuthConfig {
            private_key_path: temp_dir.path().join("id_ed25519"),
            authorized_keys_path: temp_dir.path().join("authorized_keys"),
            generate_if_missing: true,
        };

        let auth = SshAuthenticator::new(config).await.unwrap();
        (auth, temp_dir)
    }

    #[tokio::test]
    async fn test_authenticator_creation() {
        let (auth, _temp_dir) = create_test_authenticator().await;
        let public_key = auth.get_public_key().await.unwrap();
        assert!(!public_key.fingerprint().is_empty());
    }

    #[tokio::test]
    async fn test_token_generation() {
        let (auth, _temp_dir) = create_test_authenticator().await;

        // Add self to authorized keys for testing
        let public_key = auth.get_public_key().await.unwrap();
        {
            let mut authorized_keys = auth.authorized_keys.write().await;
            authorized_keys.add_key(crate::auth::AuthorizedKey {
                public_key: public_key.clone(),
                comment: Some("test".to_string()),
                options: vec![],
            });
        }

        // Generate token
        let token = auth.authenticate_peer(&public_key).await.unwrap();
        assert!(!token.token_id.is_empty());
        assert_eq!(token.peer_fingerprint, public_key.fingerprint());
        assert!(!token.is_expired());
    }

    #[tokio::test]
    async fn test_unauthorized_peer() {
        let (auth, _temp_dir) = create_test_authenticator().await;

        // Create a different key
        let other_key = crate::auth::KeyPair::generate(crate::auth::KeyType::Ed25519)
            .unwrap()
            .public_key();

        // Should fail authentication
        let result = auth.authenticate_peer(&other_key).await;
        assert!(matches!(result, Err(AuthError::UnauthorizedPeer(_))));
    }
}
