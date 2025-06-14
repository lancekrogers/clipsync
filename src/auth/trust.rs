//! Trust management for peer authentication
//!
//! Implements a Trust On First Use (TOFU) model for device authentication

use crate::auth::{AuthError, PublicKey};
use crate::discovery::PeerInfo;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Trust decision for a peer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDecision {
    /// Trust this peer
    Trust,
    /// Reject this peer
    Reject,
    /// Ignore for now (ask again later)
    Ignore,
}

/// Trust status of a peer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustStatus {
    /// Peer ID
    pub peer_id: Uuid,
    /// Peer name/hostname
    pub peer_name: String,
    /// Public key fingerprint
    pub fingerprint: String,
    /// When this peer was first seen
    pub first_seen: i64,
    /// When this peer was trusted (if trusted)
    pub trusted_at: Option<i64>,
    /// Whether this peer is trusted
    pub is_trusted: bool,
}

/// Trust manager for handling peer authentication
pub struct TrustManager {
    /// Path to trust database
    trust_db_path: PathBuf,
    /// In-memory cache of trust decisions
    trust_cache: Arc<RwLock<HashMap<String, TrustStatus>>>,
    /// Callback for prompting user
    prompt_callback: Arc<dyn Fn(&PeerInfo, &str) -> TrustDecision + Send + Sync>,
}

impl TrustManager {
    /// Create a new trust manager with default CLI prompt
    pub fn new(config_dir: PathBuf) -> Result<Self> {
        let trust_db_path = config_dir.join("trusted_devices.json");

        // Default CLI prompt implementation
        let prompt_callback = Arc::new(|peer: &PeerInfo, fingerprint: &str| -> TrustDecision {
            Self::cli_trust_prompt(peer, fingerprint)
        });

        Ok(Self {
            trust_db_path,
            trust_cache: Arc::new(RwLock::new(HashMap::new())),
            prompt_callback,
        })
    }

    /// Create with custom prompt callback (for GUI applications)
    pub fn with_prompt_callback<F>(config_dir: PathBuf, callback: F) -> Result<Self>
    where
        F: Fn(&PeerInfo, &str) -> TrustDecision + Send + Sync + 'static,
    {
        let trust_db_path = config_dir.join("trusted_devices.json");

        Ok(Self {
            trust_db_path,
            trust_cache: Arc::new(RwLock::new(HashMap::new())),
            prompt_callback: Arc::new(callback),
        })
    }

    /// Load trust database from disk
    pub async fn load(&self) -> Result<()> {
        if !self.trust_db_path.exists() {
            debug!("Trust database not found, starting fresh");
            return Ok(());
        }

        let content = tokio::fs::read_to_string(&self.trust_db_path).await?;
        let trust_map: HashMap<String, TrustStatus> = serde_json::from_str(&content)?;

        *self.trust_cache.write().await = trust_map;
        info!(
            "Loaded {} trusted devices",
            self.trust_cache.read().await.len()
        );

        Ok(())
    }

    /// Save trust database to disk
    async fn save(&self) -> Result<()> {
        // Ensure directory exists
        if let Some(parent) = self.trust_db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let trust_map = self.trust_cache.read().await;
        let content = serde_json::to_string_pretty(&*trust_map)?;
        tokio::fs::write(&self.trust_db_path, content).await?;

        Ok(())
    }

    /// Check if a peer is trusted
    pub async fn is_trusted(&self, fingerprint: &str) -> bool {
        let cache = self.trust_cache.read().await;
        cache
            .get(fingerprint)
            .map(|status| status.is_trusted)
            .unwrap_or(false)
    }

    /// Process a new peer discovery
    pub async fn process_peer(&self, peer: &PeerInfo, public_key: &PublicKey) -> Result<bool> {
        let fingerprint = public_key.fingerprint();

        // Check if already trusted
        if self.is_trusted(&fingerprint).await {
            debug!("Peer {} already trusted", peer.name);
            return Ok(true);
        }

        // Check if we've seen this peer before
        let mut cache = self.trust_cache.write().await;
        if let Some(status) = cache.get_mut(&fingerprint) {
            if !status.is_trusted {
                debug!("Peer {} was previously rejected", peer.name);
                return Ok(false);
            }
        }

        // New peer - prompt user
        drop(cache); // Release lock before prompting

        info!("New device discovered: {} ({})", peer.name, peer.id);
        let decision = (self.prompt_callback)(peer, &fingerprint);

        match decision {
            TrustDecision::Trust => {
                self.trust_peer(peer, &fingerprint).await?;
                Ok(true)
            }
            TrustDecision::Reject => {
                self.reject_peer(peer, &fingerprint).await?;
                Ok(false)
            }
            TrustDecision::Ignore => {
                debug!("User chose to ignore peer {} for now", peer.name);
                Ok(false)
            }
        }
    }

    /// Trust a peer
    async fn trust_peer(&self, peer: &PeerInfo, fingerprint: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();

        let status = TrustStatus {
            peer_id: peer.id,
            peer_name: peer.name.clone(),
            fingerprint: fingerprint.to_string(),
            first_seen: now,
            trusted_at: Some(now),
            is_trusted: true,
        };

        self.trust_cache
            .write()
            .await
            .insert(fingerprint.to_string(), status);
        self.save().await?;

        info!("Trusted peer: {} ({})", peer.name, fingerprint);
        Ok(())
    }

    /// Reject a peer
    async fn reject_peer(&self, peer: &PeerInfo, fingerprint: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();

        let status = TrustStatus {
            peer_id: peer.id,
            peer_name: peer.name.clone(),
            fingerprint: fingerprint.to_string(),
            first_seen: now,
            trusted_at: None,
            is_trusted: false,
        };

        self.trust_cache
            .write()
            .await
            .insert(fingerprint.to_string(), status);
        self.save().await?;

        warn!("Rejected peer: {} ({})", peer.name, fingerprint);
        Ok(())
    }

    /// Remove a trusted peer
    pub async fn revoke_trust(&self, fingerprint: &str) -> Result<()> {
        let mut cache = self.trust_cache.write().await;
        if cache.remove(fingerprint).is_some() {
            drop(cache);
            self.save().await?;
            info!("Revoked trust for peer with fingerprint: {}", fingerprint);
        }
        Ok(())
    }

    /// Get all trusted peers
    pub async fn get_trusted_peers(&self) -> Vec<TrustStatus> {
        self.trust_cache
            .read()
            .await
            .values()
            .filter(|s| s.is_trusted)
            .cloned()
            .collect()
    }

    /// CLI trust prompt implementation
    fn cli_trust_prompt(peer: &PeerInfo, fingerprint: &str) -> TrustDecision {
        use std::io::{self, Write};

        println!("\n=== New Device Discovered ===");
        println!("Device Name: {}", peer.name);
        println!("Device ID: {}", peer.id);
        println!(
            "Address: {}",
            peer.best_address()
                .map(|a| a.to_string())
                .unwrap_or_else(|| "Unknown".to_string())
        );
        println!("SSH Fingerprint: {}", fingerprint);
        println!("\nDo you want to trust this device?");
        println!("  [y] Yes, trust this device");
        println!("  [n] No, reject this device");
        println!("  [i] Ignore for now (ask again later)");
        print!("\nYour choice [y/n/i]: ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            return TrustDecision::Ignore;
        }

        match input.trim().to_lowercase().as_str() {
            "y" | "yes" => TrustDecision::Trust,
            "n" | "no" => TrustDecision::Reject,
            _ => TrustDecision::Ignore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_trust_manager() {
        let temp_dir = TempDir::new().unwrap();
        let manager = TrustManager::with_prompt_callback(temp_dir.path().to_path_buf(), |_, _| {
            TrustDecision::Trust
        })
        .unwrap();

        // Test initial state
        assert!(!manager.is_trusted("test-fingerprint").await);

        // Create test peer
        let peer = PeerInfo {
            id: Uuid::new_v4(),
            name: "test-device".to_string(),
            addresses: vec![],
            port: 8484,
            version: "1.0.0".to_string(),
            platform: "test".to_string(),
            metadata: Default::default(),
            last_seen: 0,
        };

        let public_key = PublicKey::from_openssh(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITestKey test@example.com",
        )
        .unwrap();

        // Process peer (should be trusted due to callback)
        let trusted = manager.process_peer(&peer, &public_key).await.unwrap();
        assert!(trusted);

        // Verify it's now trusted
        assert!(manager.is_trusted(&public_key.fingerprint()).await);

        // Verify persistence
        manager.load().await.unwrap();
        assert!(manager.is_trusted(&public_key.fingerprint()).await);
    }
}
