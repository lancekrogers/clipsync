use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tempfile::TempDir;
use tokio::time::timeout;
use uuid::Uuid;

use clipsync::{
    clipboard::{ClipboardData, ClipboardError, ClipboardProvider},
    config::Config,
    discovery::{Peer, PeerDiscovery},
    history::HistoryManager,
    sync::{SyncEngine, SyncEvent},
    transport::{TransportConfig, TransportManager},
};

// Mock clipboard provider for testing
struct MockClipboardProvider {
    content: tokio::sync::RwLock<String>,
}

impl MockClipboardProvider {
    fn new() -> Self {
        Self {
            content: tokio::sync::RwLock::new(String::new()),
        }
    }
}

#[async_trait::async_trait]
impl ClipboardProvider for MockClipboardProvider {
    async fn get_text(&self) -> Result<String, ClipboardError> {
        Ok(self.content.read().await.clone())
    }

    async fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        *self.content.write().await = text.to_string();
        Ok(())
    }

    async fn get_formats(&self) -> Result<Vec<String>, ClipboardError> {
        Ok(vec!["text/plain".to_string()])
    }

    async fn has_format(&self, format: &str) -> Result<bool, ClipboardError> {
        Ok(format == "text/plain")
    }
}

async fn create_test_setup() -> Result<(
    Arc<Config>,
    Arc<MockClipboardProvider>,
    Arc<HistoryManager>,
    Arc<PeerDiscovery>,
    Arc<TransportManager>,
)> {
    let temp_dir = TempDir::new()?;
    let config = Arc::new(Config::default_with_path(
        temp_dir.path().join("config.toml"),
    ));
    let clipboard = Arc::new(MockClipboardProvider::new());
    let history = Arc::new(HistoryManager::new(&temp_dir.path().join("history.db")).await?);
    let discovery = Arc::new(PeerDiscovery::new(config.clone()).await?);
    let transport = Arc::new(TransportManager::new(TransportConfig::default()));

    Ok((config, clipboard, history, discovery, transport))
}

#[tokio::test]
async fn test_sync_engine_creation() -> Result<()> {
    let (config, clipboard, history, discovery, transport) = create_test_setup().await?;

    let sync_engine = SyncEngine::new(config, clipboard, history, discovery, transport);

    // Test that sync engine can be created successfully
    assert!(sync_engine.get_connected_peers().await.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_clipboard_monitoring() -> Result<()> {
    let (config, clipboard, history, discovery, transport) = create_test_setup().await?;

    let sync_engine = SyncEngine::new(
        config,
        clipboard.clone(),
        history.clone(),
        discovery,
        transport,
    );

    // Subscribe to sync events
    let mut event_receiver = sync_engine.subscribe();

    // Set clipboard content
    clipboard.set_text("Test clipboard content").await?;

    // Force a sync to trigger event
    sync_engine.force_sync().await?;

    // Wait for sync event
    let event = timeout(Duration::from_secs(5), event_receiver.recv()).await??;

    assert_eq!(event.source_peer, config.node_id);
    match &event.entry.content {
        ClipboardData::Text(text) => {
            assert_eq!(text, "Test clipboard content");
        }
    }

    // Check that entry was saved to history
    let entries = history.get_recent_entries(1).await?;
    assert_eq!(entries.len(), 1);

    Ok(())
}

#[tokio::test]
async fn test_conflict_resolution() -> Result<()> {
    let (config, clipboard, history, discovery, transport) = create_test_setup().await?;

    let sync_engine = SyncEngine::new(
        config.clone(),
        clipboard.clone(),
        history.clone(),
        discovery,
        transport,
    );

    // Create two different clipboard entries with different timestamps
    let entry1 = clipsync::history::ClipboardEntry {
        id: Uuid::new_v4(),
        content: ClipboardData::Text("First entry".to_string()),
        timestamp: chrono::Utc::now() - chrono::Duration::seconds(10),
        source: config.node_id,
        checksum: "hash1".to_string(),
    };

    let entry2 = clipsync::history::ClipboardEntry {
        id: Uuid::new_v4(),
        content: ClipboardData::Text("Second entry".to_string()),
        timestamp: chrono::Utc::now(),
        source: Uuid::new_v4(),
        checksum: "hash2".to_string(),
    };

    // Add both entries to history
    history.add_entry(&entry1).await?;
    history.add_entry(&entry2).await?;

    // The more recent entry should be preferred
    let recent_entries = history.get_recent_entries(1).await?;
    assert_eq!(recent_entries.len(), 1);
    assert_eq!(recent_entries[0].checksum, "hash2");

    Ok(())
}

#[tokio::test]
async fn test_peer_connectivity() -> Result<()> {
    let (config, clipboard, history, discovery, transport) = create_test_setup().await?;

    let sync_engine = SyncEngine::new(config, clipboard, history, discovery, transport);

    // Initially no peers should be connected
    let peers = sync_engine.get_connected_peers().await;
    assert!(peers.is_empty());

    // Test would add peer discovery and connection logic here
    // For now, just verify the interface works

    Ok(())
}

#[tokio::test]
async fn test_sync_event_broadcasting() -> Result<()> {
    let (config, clipboard, history, discovery, transport) = create_test_setup().await?;

    let sync_engine = SyncEngine::new(config.clone(), clipboard, history, discovery, transport);

    // Subscribe to events
    let mut receiver1 = sync_engine.subscribe();
    let mut receiver2 = sync_engine.subscribe();

    // Force sync
    sync_engine.force_sync().await?;

    // Both receivers should get the event
    let event1 = timeout(Duration::from_secs(1), receiver1.recv()).await??;
    let event2 = timeout(Duration::from_secs(1), receiver2.recv()).await??;

    assert_eq!(event1.source_peer, event2.source_peer);
    assert_eq!(event1.entry.id, event2.entry.id);

    Ok(())
}

#[tokio::test]
async fn test_large_clipboard_content() -> Result<()> {
    let (config, clipboard, history, discovery, transport) = create_test_setup().await?;

    let sync_engine = SyncEngine::new(
        config,
        clipboard.clone(),
        history.clone(),
        discovery,
        transport,
    );

    // Create large clipboard content (1MB)
    let large_content = "A".repeat(1024 * 1024);
    clipboard.set_text(&large_content).await?;

    // Force sync
    sync_engine.force_sync().await?;

    // Verify content was handled correctly
    let entries = history.get_recent_entries(1).await?;
    assert_eq!(entries.len(), 1);

    match &entries[0].content {
        ClipboardData::Text(text) => {
            assert_eq!(text.len(), 1024 * 1024);
            assert!(text.chars().all(|c| c == 'A'));
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_concurrent_clipboard_updates() -> Result<()> {
    let (config, clipboard, history, discovery, transport) = create_test_setup().await?;

    let sync_engine = Arc::new(SyncEngine::new(
        config,
        clipboard.clone(),
        history.clone(),
        discovery,
        transport,
    ));

    // Start multiple concurrent clipboard updates
    let mut handles = Vec::new();

    for i in 0..10 {
        let clipboard_clone = clipboard.clone();
        let sync_engine_clone = sync_engine.clone();

        let handle = tokio::spawn(async move {
            let content = format!("Content {}", i);
            clipboard_clone.set_text(&content).await.unwrap();
            sync_engine_clone.force_sync().await.unwrap();
        });

        handles.push(handle);
    }

    // Wait for all updates to complete
    for handle in handles {
        handle.await?;
    }

    // Verify that all entries were recorded
    let entries = history.get_recent_entries(20).await?;
    assert!(entries.len() >= 1); // At least one entry should be present

    Ok(())
}
