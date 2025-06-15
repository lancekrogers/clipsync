use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

use crate::config::Config;
use crate::discovery::Discovery;

// Adapter types for the new sync engine interface

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClipboardData {
    Text(String),
}

#[derive(Debug, Clone)]
pub struct ClipboardEntry {
    pub id: Uuid,
    pub content: ClipboardData,
    pub timestamp: DateTime<Utc>,
    pub source: Uuid,
    pub checksum: String,
}

#[derive(Debug, Clone)]
pub struct Peer {
    pub id: Uuid,
    pub hostname: String,
    pub address: String,
}

pub struct AuthenticatedConnection {
    pub peer_id: Uuid,
}

impl AuthenticatedConnection {
    pub async fn authenticate(&self) -> Result<AuthResult> {
        Ok(AuthResult {
            authenticated: true,
        })
    }
}

pub struct AuthResult {
    pub authenticated: bool,
}

impl AuthResult {
    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }
}

// HistoryManager adapter
pub struct HistoryManager {
    inner: crate::history::ClipboardHistory,
}

impl HistoryManager {
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let inner = crate::history::ClipboardHistory::new(path.as_ref()).await?;
        Ok(Self { inner })
    }

    pub async fn add_entry(&self, entry: &ClipboardEntry) -> Result<()> {
        let content = match &entry.content {
            ClipboardData::Text(text) => text.as_bytes().to_vec(),
        };

        let history_content = crate::history::ClipboardContent {
            id: entry.id,
            content,
            content_type: "text/plain".to_string(),
            timestamp: entry.timestamp.timestamp(),
            origin_node: entry.source,
        };

        self.inner.add(&history_content).await
    }

    pub async fn get_recent_entries(&self, limit: usize) -> Result<Vec<ClipboardEntry>> {
        let entries = self.inner.get_recent(limit).await?;

        Ok(entries
            .into_iter()
            .map(|entry| {
                let content = match String::from_utf8(entry.content) {
                    Ok(text) => ClipboardData::Text(text),
                    Err(_) => ClipboardData::Text("[Binary Data]".to_string()),
                };

                ClipboardEntry {
                    id: entry.id,
                    content,
                    timestamp: DateTime::from_timestamp(entry.timestamp, 0)
                        .unwrap_or_else(Utc::now),
                    source: entry.origin_node,
                    checksum: entry.checksum,
                }
            })
            .collect())
    }

    pub async fn get_by_checksum(&self, checksum: &str) -> Result<Option<ClipboardEntry>> {
        // Simplified implementation - would need to be enhanced
        let entries = self.get_recent_entries(100).await?;
        Ok(entries.into_iter().find(|e| e.checksum == checksum))
    }

    pub async fn search_entries(&self, search_term: &str, limit: usize) -> Result<Vec<ClipboardEntry>> {
        let inner_entries = self.inner.search(search_term).await?;
        
        let entries = inner_entries
            .into_iter()
            .take(limit)
            .map(|entry| {
                let content = match String::from_utf8(entry.content) {
                    Ok(text) => ClipboardData::Text(text),
                    Err(_) => ClipboardData::Text("[Binary Data]".to_string()),
                };

                ClipboardEntry {
                    id: entry.id,
                    content,
                    timestamp: DateTime::from_timestamp(entry.timestamp, 0)
                        .unwrap_or_else(Utc::now),
                    source: entry.origin_node,
                    checksum: entry.checksum,
                }
            })
            .collect();

        Ok(entries)
    }
}

// ClipboardProvider wrapper to add text methods
pub struct ClipboardProviderWrapper {
    inner: Box<dyn crate::clipboard::ClipboardProvider>,
}

impl ClipboardProviderWrapper {
    pub fn new(inner: Box<dyn crate::clipboard::ClipboardProvider>) -> Self {
        Self { inner }
    }

    pub async fn get_text(&self) -> Result<String> {
        let content = self.inner.get_content().await?;
        Ok(String::from_utf8_lossy(&content.data).to_string())
    }

    pub async fn set_text(&self, text: &str) -> Result<()> {
        let content = crate::clipboard::ClipboardContent::text(text);
        self.inner.set_content(&content).await?;
        Ok(())
    }

    pub async fn clear(&self) -> Result<()> {
        self.inner.clear().await?;
        Ok(())
    }
}

// ClipboardProvider getter function
pub async fn get_clipboard_provider() -> Result<ClipboardProviderWrapper> {
    #[cfg(target_os = "macos")]
    {
        let provider = Box::new(crate::clipboard::macos::MacOSClipboard::new()?);
        Ok(ClipboardProviderWrapper::new(provider))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Try X11 first, then Wayland
        let provider = if let Ok(provider) = crate::clipboard::x11::X11Clipboard::new() {
            Box::new(provider) as Box<dyn crate::clipboard::ClipboardProvider>
        } else {
            Box::new(crate::clipboard::wayland::WaylandClipboard::new().await?)
        };
        Ok(ClipboardProviderWrapper::new(provider))
    }

    #[cfg(windows)]
    {
        compile_error!("Windows clipboard support not implemented yet");
    }
}

// PeerDiscovery adapter
pub struct PeerDiscovery {
    inner: Arc<tokio::sync::Mutex<crate::discovery::DiscoveryService>>,
    event_tx: tokio::sync::broadcast::Sender<Peer>,
    discovery_event_tx: tokio::sync::broadcast::Sender<crate::discovery::DiscoveryEvent>,
    config: Arc<Config>,
}

impl PeerDiscovery {
    pub async fn new(config: Arc<Config>) -> Result<Self> {
        let inner = crate::discovery::DiscoveryService::new(&config)?;
        let (event_tx, _) = tokio::sync::broadcast::channel(100);
        let (discovery_event_tx, _) = tokio::sync::broadcast::channel(100);
        Ok(Self {
            inner: Arc::new(tokio::sync::Mutex::new(inner)),
            event_tx,
            discovery_event_tx,
            config,
        })
    }

    pub async fn start(&self) -> Result<()> {
        // Load public key for announcement
        let public_key = self
            .config
            .auth
            .load_public_key()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to load public key: {}", e))?;

        // Create service info with public key
        let mut service_info = crate::discovery::ServiceInfo::from_config(
            self.config.node_id(),
            8484, // TODO: Get from config
        );

        // Add public key to TXT records
        service_info
            .txt_data
            .push(("pubkey".to_string(), public_key));

        // Start discovery and announce
        let mut inner = self.inner.lock().await;
        inner.start().await?;
        inner.announce(service_info).await?;
        drop(inner); // Release lock

        // Start event forwarding task
        let inner_clone = Arc::clone(&self.inner);
        let event_tx = self.event_tx.clone();
        let discovery_event_tx = self.discovery_event_tx.clone();
        let config = Arc::clone(&self.config);

        tokio::spawn(async move {
            // Get a receiver once at the start
            let mut event_rx = {
                let mut inner = inner_clone.lock().await;
                inner.subscribe_changes()
            };

            loop {
                match event_rx.recv().await {
                    Some(event) => {
                        // Forward the raw discovery event
                        let _ = discovery_event_tx.send(event.clone());

                        match event {
                            crate::discovery::DiscoveryEvent::PeerDiscovered(peer_info) => {
                                let peer = Peer {
                                    id: peer_info.id,
                                    hostname: peer_info.name.clone(),
                                    address: peer_info
                                        .best_address()
                                        .map(|a| a.to_string())
                                        .unwrap_or_else(|| "unknown".to_string()),
                                };
                                let _ = event_tx.send(peer);
                            }
                            crate::discovery::DiscoveryEvent::PeerUpdated(peer_info) => {
                                // Handle peer updates
                                tracing::debug!("Peer updated: {}", peer_info.name);
                            }
                            crate::discovery::DiscoveryEvent::PeerLost(peer_id) => {
                                tracing::debug!("Peer lost: {}", peer_id);
                            }
                            crate::discovery::DiscoveryEvent::Error(err) => {
                                tracing::warn!("Discovery error: {}", err);
                            }
                        }
                    }
                    None => {
                        // Channel closed, try to get a new receiver
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        match inner_clone.lock().await.subscribe_changes().recv().await {
                            Some(_) => {
                                // Got new receiver, continue
                                event_rx = inner_clone.lock().await.subscribe_changes();
                            }
                            None => {
                                // Discovery service stopped
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok(())
    }

    pub async fn subscribe(&self) -> Result<tokio::sync::broadcast::Receiver<Peer>> {
        Ok(self.event_tx.subscribe())
    }

    /// Get a receiver for raw discovery events (for trust processing)
    pub fn get_discovery_event_receiver(
        &self,
    ) -> tokio::sync::broadcast::Receiver<crate::discovery::DiscoveryEvent> {
        self.discovery_event_tx.subscribe()
    }

    pub async fn discover_peers_timeout(&self, timeout: std::time::Duration) -> Result<Vec<crate::discovery::PeerInfo>> {
        let mut inner = self.inner.lock().await;
        
        // Start the discovery process
        inner.start().await?;
        
        // Wait for the timeout period
        tokio::time::sleep(timeout).await;
        
        // Get discovered peers
        let peers = inner.discover_peers().await?;
        
        Ok(peers)
    }
}

// Config extensions
impl Config {
    pub fn node_id(&self) -> Uuid {
        self.node_id
    }

    pub fn sync_interval_ms(&self) -> u64 {
        1000 // Default 1 second
    }

    pub fn database_path(&self) -> std::path::PathBuf {
        self.clipboard.history_db.clone()
    }

    pub fn websocket_port(&self) -> u16 {
        // Extract port from listen_addr or use default
        if let Some(port_str) = self.listen_addr.split(':').last() {
            port_str.parse().unwrap_or(8484)
        } else {
            8484
        }
    }

    pub async fn save_config(&self) -> Result<()> {
        self.save()
            .map_err(|e| anyhow::anyhow!("Failed to save config: {}", e))
    }

    pub fn default_with_path(_path: std::path::PathBuf) -> Self {
        Self::default()
    }
}
