//! Integration between discovery and trust management

use crate::auth::{Authenticator, PublicKey, TrustManager};
use crate::discovery::{DiscoveryEvent, PeerInfo};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Trust-aware discovery wrapper
pub struct TrustAwareDiscovery {
    /// Trust manager
    trust_manager: Arc<TrustManager>,
    /// SSH authenticator
    ssh_auth: Arc<crate::auth::SshAuthenticator>,
}

impl TrustAwareDiscovery {
    /// Create a new trust-aware discovery
    pub fn new(
        trust_manager: Arc<TrustManager>,
        ssh_auth: Arc<crate::auth::SshAuthenticator>,
    ) -> Self {
        Self {
            trust_manager,
            ssh_auth,
        }
    }

    /// Process discovery events with trust management
    pub async fn process_discovery_events(
        &self,
        mut event_rx: mpsc::Receiver<DiscoveryEvent>,
    ) -> Result<()> {
        while let Some(event) = event_rx.recv().await {
            match event {
                DiscoveryEvent::PeerDiscovered(peer_info) => {
                    self.process_new_peer(&peer_info).await?;
                }
                DiscoveryEvent::PeerUpdated(peer_info) => {
                    debug!("Peer updated: {}", peer_info.name);
                    // Check if trust status changed
                    self.process_peer_update(&peer_info).await?;
                }
                DiscoveryEvent::PeerLost(peer_id) => {
                    debug!("Peer lost: {}", peer_id);
                }
                DiscoveryEvent::Error(err) => {
                    warn!("Discovery error: {}", err);
                }
            }
        }
        Ok(())
    }

    /// Process a newly discovered peer
    async fn process_new_peer(&self, peer_info: &PeerInfo) -> Result<()> {
        info!("Processing new peer: {} ({})", peer_info.name, peer_info.id);

        // Extract public key from metadata
        let public_key = match &peer_info.metadata.ssh_public_key {
            Some(key_str) => match PublicKey::from_openssh(key_str) {
                Ok(key) => key,
                Err(e) => {
                    warn!("Invalid public key from peer {}: {}", peer_info.name, e);
                    return Ok(());
                }
            },
            None => {
                warn!("Peer {} has no public key in metadata", peer_info.name);
                return Ok(());
            }
        };

        // Check if already authorized
        if self.ssh_auth.is_authorized(&public_key).await? {
            debug!("Peer {} is already authorized", peer_info.name);
            return Ok(());
        }

        // Process through trust manager
        let trusted = self
            .trust_manager
            .process_peer(peer_info, &public_key)
            .await?;

        if trusted {
            // Add to authorized_keys
            info!("Adding trusted peer {} to authorized_keys", peer_info.name);
            let comment = Some(format!("ClipSync: {} ({})", peer_info.name, peer_info.id));
            self.ssh_auth
                .add_trusted_peer(
                    &peer_info.metadata.ssh_public_key.as_ref().unwrap(),
                    comment,
                )
                .await?;
        }

        Ok(())
    }

    /// Process an updated peer
    async fn process_peer_update(&self, peer_info: &PeerInfo) -> Result<()> {
        // Check if public key changed
        if let Some(key_str) = &peer_info.metadata.ssh_public_key {
            if let Ok(public_key) = PublicKey::from_openssh(key_str) {
                let fingerprint = public_key.fingerprint();

                // Check if trust status needs update
                let is_trusted = self.trust_manager.is_trusted(&fingerprint).await;
                let is_authorized = self.ssh_auth.is_authorized(&public_key).await?;

                if is_trusted && !is_authorized {
                    // Re-add to authorized_keys
                    info!(
                        "Re-adding trusted peer {} to authorized_keys",
                        peer_info.name
                    );
                    let comment = Some(format!("ClipSync: {} ({})", peer_info.name, peer_info.id));
                    self.ssh_auth.add_trusted_peer(key_str, comment).await?;
                } else if !is_trusted && is_authorized {
                    // Remove from authorized_keys
                    warn!(
                        "Removing untrusted peer {} from authorized_keys",
                        peer_info.name
                    );
                    self.ssh_auth.remove_peer(&fingerprint).await?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_trust_aware_discovery() {
        let temp_dir = TempDir::new().unwrap();

        // Create trust manager
        let trust_manager = Arc::new(
            TrustManager::with_prompt_callback(temp_dir.path().to_path_buf(), |_, _| {
                crate::auth::TrustDecision::Trust
            })
            .unwrap(),
        );

        // Create SSH authenticator
        let auth_config = crate::auth::AuthConfig {
            private_key_path: temp_dir.path().join("id_ed25519"),
            authorized_keys_path: temp_dir.path().join("authorized_keys"),
            generate_if_missing: true,
        };
        let ssh_auth = Arc::new(
            crate::auth::SshAuthenticator::new(auth_config)
                .await
                .unwrap(),
        );

        // Create trust-aware discovery
        let discovery = TrustAwareDiscovery::new(trust_manager, ssh_auth);

        // Create test peer with public key
        let test_key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITestKey test@example.com";
        let peer_info = PeerInfo {
            id: Uuid::new_v4(),
            name: "test-peer".to_string(),
            addresses: vec![],
            port: 8484,
            version: "1.0.0".to_string(),
            platform: "test".to_string(),
            metadata: crate::discovery::PeerMetadata {
                ssh_public_key: Some(test_key.to_string()),
                ..Default::default()
            },
            last_seen: 0,
        };

        // Process new peer
        discovery.process_new_peer(&peer_info).await.unwrap();

        // Verify it was added to authorized_keys
        let public_key = PublicKey::from_openssh(test_key).unwrap();
        assert!(discovery.ssh_auth.is_authorized(&public_key).await.unwrap());
    }
}
