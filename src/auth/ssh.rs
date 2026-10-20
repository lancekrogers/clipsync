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
    /// Trust manager
    trust_manager: Option<Arc<crate::auth::TrustManager>>,
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
            trust_manager: None,
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

    /// Set the trust manager
    pub fn set_trust_manager(&mut self, trust_manager: Arc<crate::auth::TrustManager>) {
        self.trust_manager = Some(trust_manager);
    }

    /// Add a trusted peer's public key to authorized_keys.
    /// Uses the shared flock + atomic rename path so concurrent CLI/daemon
    /// updates cannot lose unrelated entries.
    pub async fn add_trusted_peer(
        &self,
        openssh_key: &str,
        comment: Option<String>,
    ) -> Result<(), AuthError> {
        self.add_trusted_peer_with_commit_hooks(openssh_key, comment, || Ok(()), || {}, || {})
            .await
    }

    /// Add a trusted peer with pre-publish and post-save hooks under the flock.
    ///
    /// `before_mutate` runs under flock before any key change (deadline/cancel).
    /// `after_save` runs under flock only after durable rename (point of no return).
    /// `on_save_fail` reopens a commit attempt if the rename fails.
    pub async fn add_trusted_peer_with_commit_hooks<B, A, E>(
        &self,
        openssh_key: &str,
        comment: Option<String>,
        before_mutate: B,
        after_save: A,
        on_save_fail: E,
    ) -> Result<(), AuthError>
    where
        B: FnOnce() -> Result<(), AuthError> + Send + 'static,
        A: FnOnce() + Send + 'static,
        E: FnOnce() + Send + 'static,
    {
        let path = self.config.authorized_keys_path.clone();
        let openssh_key = openssh_key.to_string();
        let comment = sanitize_authorized_comment(comment)?;
        // Hold in-memory write lock across the file RMW so readers see a
        // consistent snapshot and lost updates cannot race this process.
        let mut guard = self.authorized_keys.write().await;
        let updated = crate::auth::AuthorizedKeys::update_file_with_hooks(
            &path,
            move |keys| {
                before_mutate()?;
                keys.add_key_from_openssh(&openssh_key, comment)?;
                Ok(keys.clone_keys())
            },
            after_save,
            on_save_fail,
        )
        .await?;
        *guard = updated;
        Ok(())
    }

    /// Remove a peer from authorized_keys (offline revoke). Preserves unrelated pins.
    pub async fn remove_peer(&self, fingerprint: &str) -> Result<bool, AuthError> {
        let path = self.config.authorized_keys_path.clone();
        let fingerprint = fingerprint.to_string();
        let mut guard = self.authorized_keys.write().await;
        let (removed, updated) = crate::auth::AuthorizedKeys::update_file(&path, move |keys| {
            let removed = keys.remove_key_by_fingerprint(&fingerprint);
            Ok((removed, keys.clone_keys()))
        })
        .await?;
        *guard = updated;
        Ok(removed)
    }

    /// Snapshot of pinned devices for inventory / TrustedDevices IPC.
    /// Re-reads disk so CLI `auth add|remove` in another process is visible.
    pub async fn list_trusted_devices(&self) -> Result<Vec<(String, Option<String>)>, AuthError> {
        let keys = self.reload_authorized_keys_fail_closed().await?;
        Ok(keys
            .list_keys()
            .iter()
            .map(|k| (k.public_key.fingerprint(), k.comment.clone()))
            .collect())
    }

    /// Reload pins from disk under the write lock (after external update_file).
    pub async fn reload_authorized_keys(&self) -> Result<(), AuthError> {
        let _ = self.reload_authorized_keys_fail_closed().await?;
        Ok(())
    }

    /// Cross-process coherent reload. Unreadable/invalid store fails closed.
    async fn reload_authorized_keys_fail_closed(
        &self,
    ) -> Result<crate::auth::AuthorizedKeys, AuthError> {
        let loaded =
            crate::auth::AuthorizedKeys::load_from_file(&self.config.authorized_keys_path).await?;
        *self.authorized_keys.write().await = loaded.clone_keys();
        Ok(loaded)
    }
}

/// Strip CR/LF and control bytes from authorized_keys comments; bound label size.
/// Rejects comments that would inject extra authorization lines.
pub fn sanitize_authorized_comment(comment: Option<String>) -> Result<Option<String>, AuthError> {
    let Some(raw) = comment else {
        return Ok(None);
    };
    if raw.bytes().any(|b| b == b'\n' || b == b'\r' || b == 0) {
        return Err(AuthError::InvalidKeyFormat(
            "authorized_keys comment must not contain CR/LF/NUL".into(),
        ));
    }
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_COMMENT_CHARS)
        .collect();
    if cleaned.is_empty() {
        Ok(None)
    } else {
        Ok(Some(cleaned))
    }
}

const MAX_COMMENT_CHARS: usize = 128;

#[async_trait]
impl Authenticator for SshAuthenticator {
    async fn identity_pkcs8(&self) -> Result<Vec<u8>, AuthError> {
        let key = self.key_pair.read().await;
        Ok(key
            .as_ref()
            .ok_or_else(|| AuthError::KeyError("No local identity".into()))?
            .pkcs8())
    }
    async fn trusted_keys(&self) -> Result<Vec<PublicKey>, AuthError> {
        // Re-read authorized_keys so a separate CLI/daemon process that used
        // update_file is reflected immediately. Fail closed on IO/parse errors.
        let keys = self.reload_authorized_keys_fail_closed().await?;
        Ok(keys
            .list_keys()
            .iter()
            .map(|k| k.public_key.clone())
            .collect())
    }

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
        let keys = self.reload_authorized_keys_fail_closed().await?;
        Ok(keys.is_authorized(peer_key))
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
        auth.add_trusted_peer(&public_key.to_openssh(), Some("test".into()))
            .await
            .unwrap();

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

    #[tokio::test]
    async fn separate_authenticator_instances_see_add_and_remove() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("authorized_keys");
        let key_path = temp_dir.path().join("id_ed25519");
        let config = AuthConfig {
            private_key_path: key_path.clone(),
            authorized_keys_path: path.clone(),
            generate_if_missing: true,
        };
        let writer = SshAuthenticator::new(config.clone()).await.unwrap();
        let reader = SshAuthenticator::new(AuthConfig {
            private_key_path: key_path,
            authorized_keys_path: path,
            generate_if_missing: false,
        })
        .await
        .unwrap();

        let peer = crate::auth::KeyPair::generate(crate::auth::KeyType::Ed25519)
            .unwrap()
            .public_key();
        writer
            .add_trusted_peer(&peer.to_openssh(), Some("peer".into()))
            .await
            .unwrap();
        assert!(reader.is_authorized(&peer).await.unwrap());
        assert!(reader
            .list_trusted_devices()
            .await
            .unwrap()
            .iter()
            .any(|(fp, _)| fp == &peer.fingerprint()));

        writer.remove_peer(&peer.fingerprint()).await.unwrap();
        assert!(!reader.is_authorized(&peer).await.unwrap());
    }

    #[tokio::test]
    async fn malicious_comment_with_newline_rejected() {
        let (auth, _temp_dir) = create_test_authenticator().await;
        let peer = crate::auth::KeyPair::generate(crate::auth::KeyType::Ed25519)
            .unwrap()
            .public_key();
        let evil = format!(
            "nice\nssh-ed25519 {}",
            peer.to_openssh().split_whitespace().nth(1).unwrap()
        );
        let err = auth
            .add_trusted_peer(&peer.to_openssh(), Some(evil))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidKeyFormat(_)));
        assert!(!auth.is_authorized(&peer).await.unwrap());
    }

    #[test]
    fn sanitize_rejects_crlf_injection() {
        let injected = "x\nssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl evil";
        assert!(sanitize_authorized_comment(Some(injected.into())).is_err());
        assert!(sanitize_authorized_comment(Some("ok-name".into()))
            .unwrap()
            .is_some());
    }
}
