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
    inner: tokio::sync::Mutex<crate::discovery::DiscoveryService>,
    event_tx: tokio::sync::broadcast::Sender<Peer>,
    started: std::sync::atomic::AtomicBool,
}

impl PeerDiscovery {
    pub async fn new(config: Arc<Config>) -> Result<Self> {
        let inner = crate::discovery::DiscoveryService::new(&config)?;
        let (event_tx, _) = tokio::sync::broadcast::channel(100);
        Ok(Self {
            inner: tokio::sync::Mutex::new(inner),
            event_tx,
            started: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub async fn start(&self) -> Result<()> {
        // Check if already started
        if self.started.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Ok(()); // Already started
        }

        // Start the discovery service
        let mut inner = self.inner.lock().await;
        inner.start().await?;

        // Get event receiver from the discovery service
        let mut event_rx = inner.subscribe_changes();
        drop(inner); // Release the lock

        let event_tx = self.event_tx.clone();

        // Spawn task to convert discovery events to Peer events
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                match event {
                    crate::discovery::DiscoveryEvent::PeerDiscovered(peer_info)
                    | crate::discovery::DiscoveryEvent::PeerUpdated(peer_info) => {
                        if let Some(address) = peer_info.best_address() {
                            let peer = Peer {
                                id: peer_info.id,
                                hostname: peer_info.name,
                                address: address.to_string(),
                            };
                            let _ = event_tx.send(peer);
                        }
                    }
                    crate::discovery::DiscoveryEvent::PeerLost(_) => {
                        // Could emit a peer lost event if needed
                    }
                    crate::discovery::DiscoveryEvent::Error(_) => {
                        // Could handle errors if needed
                    }
                }
            }
        });

        Ok(())
    }

    pub async fn subscribe(&self) -> Result<tokio::sync::broadcast::Receiver<Peer>> {
        Ok(self.event_tx.subscribe())
    }

    pub async fn announce(&self, node_id: Uuid, port: u16) -> Result<()> {
        let service_info = crate::discovery::ServiceInfo::from_config(node_id, port);
        let mut inner = self.inner.lock().await;
        inner.announce(service_info).await
    }

    pub async fn get_peers(&self) -> Result<Vec<Peer>> {
        let mut inner = self.inner.lock().await;
        let peer_infos = inner.discover_peers().await?;
        Ok(peer_infos
            .into_iter()
            .filter_map(|peer_info| {
                peer_info.best_address().map(|address| Peer {
                    id: peer_info.id,
                    hostname: peer_info.name,
                    address: address.to_string(),
                })
            })
            .collect())
    }

    pub async fn stop(&self) -> Result<()> {
        self.started
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let mut inner = self.inner.lock().await;
        inner.stop().await
    }
}

// Config extensions
impl Config {
    pub fn node_id(&self) -> Uuid {
        // Return the persistent node ID from config
        self.node_id
    }

    pub fn sync_interval_ms(&self) -> u64 {
        1000 // Default 1 second
    }

    pub fn database_path(&self) -> std::path::PathBuf {
        self.clipboard.history_db.clone()
    }

    pub async fn load_config(config_path: Option<std::path::PathBuf>) -> Result<Self> {
        if let Some(path) = config_path {
            Self::load_from_path(&path).map_err(|e| anyhow::anyhow!("Failed to load config: {}", e))
        } else {
            // Use Config::load() which properly expands paths
            Self::load().map_err(|e| anyhow::anyhow!("Failed to load config: {}", e))
        }
    }

    pub async fn save_config(&self) -> Result<()> {
        self.save()
            .map_err(|e| anyhow::anyhow!("Failed to save config: {}", e))
    }

    pub async fn generate_example_config(force: bool) -> Result<()> {
        let config = Self::default();
        let example_path = std::path::PathBuf::from("config.example.toml");

        if !force && example_path.exists() {
            return Err(anyhow::anyhow!(
                "Example config already exists. Use --force to overwrite."
            ));
        }

        config
            .save()
            .map_err(|e| anyhow::anyhow!("Failed to save example config: {}", e))
    }

    pub fn default_with_path(_path: std::path::PathBuf) -> Self {
        Self::default()
    }
}
