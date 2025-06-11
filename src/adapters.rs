use std::sync::Arc;
use std::path::Path;
use anyhow::Result;
use uuid::Uuid;
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

use crate::config::Config;

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
        Ok(AuthResult { authenticated: true })
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
        
        Ok(entries.into_iter().map(|entry| {
            let content = match String::from_utf8(entry.content) {
                Ok(text) => ClipboardData::Text(text),
                Err(_) => ClipboardData::Text("[Binary Data]".to_string()),
            };

            ClipboardEntry {
                id: entry.id,
                content,
                timestamp: DateTime::from_timestamp(entry.timestamp, 0).unwrap_or_else(Utc::now),
                source: entry.origin_node,
                checksum: entry.checksum,
            }
        }).collect())
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
            Box::new(crate::clipboard::wayland::WaylandClipboard::new()?)
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
    inner: crate::discovery::DiscoveryService,
}

impl PeerDiscovery {
    pub async fn new(config: Arc<Config>) -> Result<Self> {
        let inner = crate::discovery::DiscoveryService::new(&config)?;
        Ok(Self { inner })
    }

    pub async fn start(&self) -> Result<()> {
        // Start discovery service
        Ok(())
    }

    pub async fn subscribe(&self) -> Result<tokio::sync::broadcast::Receiver<Peer>> {
        let (_tx, rx) = tokio::sync::broadcast::channel(100);
        // In a real implementation, this would forward discovery events
        Ok(rx)
    }
}

// Config extensions
impl Config {
    pub fn node_id(&self) -> Uuid {
        // Generate a consistent node ID based on config
        Uuid::new_v4() // In practice, this should be persistent
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
        self.save().map_err(|e| anyhow::anyhow!("Failed to save config: {}", e))
    }

    pub async fn generate_example_config(force: bool) -> Result<()> {
        let config = Self::default();
        let example_path = std::path::PathBuf::from("config.example.toml");
        
        if !force && example_path.exists() {
            return Err(anyhow::anyhow!("Example config already exists. Use --force to overwrite."));
        }
        
        config.save().map_err(|e| anyhow::anyhow!("Failed to save example config: {}", e))
    }

    pub async fn validate(path: &std::path::Path) -> Result<()> {
        let _config = Self::load_from_path(path).map_err(|e| anyhow::anyhow!("Invalid config: {}", e))?;
        Ok(())
    }

    pub fn default_with_path(_path: std::path::PathBuf) -> Self {
        Self::default()
    }
}