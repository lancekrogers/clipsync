//! Clipboard history management and persistence

pub mod database;
pub mod encryption;

use std::path::Path;
use anyhow::Result;
use uuid::Uuid;
use serde::{Deserialize, Serialize};

/// Content to be stored in clipboard history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardContent {
    /// Unique identifier for this clipboard entry
    pub id: Uuid,
    /// The actual clipboard content
    pub content: Vec<u8>,
    /// MIME type (text/plain, text/rtf, image/png)
    pub content_type: String,
    /// Unix timestamp when content was created
    pub timestamp: i64,
    /// UUID of the node that created this content
    pub origin_node: Uuid,
}

/// Entry retrieved from clipboard history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Unique identifier
    pub id: Uuid,
    /// Decrypted content
    pub content: Vec<u8>,
    /// MIME type
    pub content_type: String,
    /// Original content size before encryption
    pub content_size: u64,
    /// Unix timestamp
    pub timestamp: i64,
    /// Node that created this entry
    pub origin_node: Uuid,
    /// SHA-256 checksum of content
    pub checksum: String,
}

/// Main interface for clipboard history management
pub struct ClipboardHistory {
    db: database::HistoryDatabase,
    encryptor: encryption::Encryptor,
}

impl ClipboardHistory {
    /// Create a new clipboard history instance
    pub async fn new(db_path: &Path) -> Result<Self> {
        let encryptor = encryption::Encryptor::new().await?;
        let db = database::HistoryDatabase::new(db_path, encryptor.get_key()).await?;
        
        Ok(Self { db, encryptor })
    }
    
    /// Add new content to history
    pub async fn add(&self, content: &ClipboardContent) -> Result<()> {
        self.db.insert(content, &self.encryptor).await
    }
    
    /// Get the most recent entries from history
    pub async fn get_recent(&self, count: usize) -> Result<Vec<HistoryEntry>> {
        self.db.get_recent(count, &self.encryptor).await
    }
    
    /// Get entry by index (0 = most recent)
    pub async fn get_by_index(&self, index: u8) -> Result<HistoryEntry> {
        self.db.get_by_index(index, &self.encryptor).await
    }
    
    /// Search text entries for matching content
    pub async fn search(&self, query: &str) -> Result<Vec<HistoryEntry>> {
        self.db.search(query, &self.encryptor).await
    }
    
    /// Clear all history entries
    pub async fn clear(&self) -> Result<()> {
        self.db.clear().await
    }
}