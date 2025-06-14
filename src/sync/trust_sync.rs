//! Trust-aware sync engine integration

use crate::adapters::{ClipboardProviderWrapper, HistoryManager, PeerDiscovery};
use crate::auth::{SshAuthenticator, TrustManager};
use crate::config::Config;
use crate::discovery::TrustAwareDiscovery;
use crate::sync::SyncEngine;
use crate::transport::TransportManager;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

/// Enhanced sync engine with trust management
pub struct TrustAwareSyncEngine {
    /// The base sync engine
    sync_engine: Arc<SyncEngine>,
    /// Trust manager
    trust_manager: Arc<TrustManager>,
    /// SSH authenticator
    ssh_auth: Arc<SshAuthenticator>,
    /// Trust-aware discovery
    trust_discovery: Arc<TrustAwareDiscovery>,
}

impl TrustAwareSyncEngine {
    /// Create a new trust-aware sync engine
    pub async fn new(
        config: Arc<Config>,
        clipboard: Arc<ClipboardProviderWrapper>,
        history: Arc<HistoryManager>,
        discovery: Arc<PeerDiscovery>,
        transport: Arc<TransportManager>,
    ) -> Result<Self> {
        // Create trust manager
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?
            .join("clipsync");

        let trust_manager = Arc::new(TrustManager::new(config_dir)?);
        trust_manager.load().await?;

        // Create SSH authenticator with trust manager
        let auth_config = crate::auth::AuthConfig {
            private_key_path: config.auth.ssh_key.clone(),
            authorized_keys_path: config.auth.authorized_keys.clone(),
            generate_if_missing: true,
        };

        let mut ssh_auth = SshAuthenticator::new(auth_config).await?;
        ssh_auth.set_trust_manager(Arc::clone(&trust_manager));
        let ssh_auth = Arc::new(ssh_auth);

        // Create trust-aware discovery
        let trust_discovery = Arc::new(TrustAwareDiscovery::new(
            Arc::clone(&trust_manager),
            Arc::clone(&ssh_auth),
        ));

        // Create base sync engine
        let sync_engine = Arc::new(SyncEngine::new(
            config, clipboard, history, discovery, transport,
        ));

        Ok(Self {
            sync_engine,
            trust_manager,
            ssh_auth,
            trust_discovery,
        })
    }

    /// Start the trust-aware sync engine
    pub async fn start(&self) -> Result<()> {
        info!("Starting trust-aware sync engine");

        // Start base sync engine
        let sync_task = {
            let sync_engine = Arc::clone(&self.sync_engine);
            tokio::spawn(async move { sync_engine.start().await })
        };

        // Wait for tasks
        sync_task.await??;

        Ok(())
    }

    /// Start trust processing in a separate task
    pub async fn start_trust_processing(&self, discovery: Arc<PeerDiscovery>) -> Result<()> {
        let trust_discovery = Arc::clone(&self.trust_discovery);
        let mut event_rx = discovery.get_discovery_event_receiver();

        tokio::spawn(async move {
            // Convert broadcast receiver to mpsc for trust processing
            let (tx, mut rx) = tokio::sync::mpsc::channel(100);

            // Forward events from broadcast to mpsc
            tokio::spawn(async move {
                loop {
                    match event_rx.recv().await {
                        Ok(event) => {
                            if tx.send(event).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Discovery event receiver lagged by {} messages", n);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            break;
                        }
                    }
                }
            });

            // Process events through trust manager
            if let Err(e) = trust_discovery.process_discovery_events(rx).await {
                tracing::error!("Error processing discovery events: {}", e);
            }
        });

        Ok(())
    }

    /// Get the SSH authenticator for other components
    pub fn ssh_authenticator(&self) -> Arc<SshAuthenticator> {
        Arc::clone(&self.ssh_auth)
    }

    /// Get the trust manager
    pub fn trust_manager(&self) -> Arc<TrustManager> {
        Arc::clone(&self.trust_manager)
    }

    /// Subscribe to sync events
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<crate::sync::SyncEvent> {
        self.sync_engine.subscribe()
    }

    /// Get connected peers
    pub async fn get_connected_peers(&self) -> Vec<crate::adapters::Peer> {
        self.sync_engine.get_connected_peers().await
    }

    /// Force sync
    pub async fn force_sync(&self) -> Result<()> {
        self.sync_engine.force_sync().await
    }
}

/// Helper to set up trust-aware sync with minimal configuration
pub async fn setup_trust_sync(
    config: Arc<Config>,
    clipboard: Arc<ClipboardProviderWrapper>,
    history: Arc<HistoryManager>,
    discovery: Arc<PeerDiscovery>,
    transport: Arc<TransportManager>,
) -> Result<TrustAwareSyncEngine> {
    TrustAwareSyncEngine::new(config, clipboard, history, discovery, transport).await
}
