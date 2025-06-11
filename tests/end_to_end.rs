use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

use clipsync::{
    config::Config,
    clipboard::{ClipboardProvider, ClipboardData, ClipboardError},
    cli::{CliHandler, Commands},
    discovery::PeerDiscovery,
    history::HistoryManager,
    sync::SyncEngine,
    transport::{TransportManager, TransportConfig},
};

// Mock clipboard provider for end-to-end testing
struct TestClipboardProvider {
    content: tokio::sync::RwLock<String>,
    change_count: tokio::sync::RwLock<usize>,
}

impl TestClipboardProvider {
    fn new() -> Self {
        Self {
            content: tokio::sync::RwLock::new(String::new()),
            change_count: tokio::sync::RwLock::new(0),
        }
    }

    async fn get_change_count(&self) -> usize {
        *self.change_count.read().await
    }
}

#[async_trait::async_trait]
impl ClipboardProvider for TestClipboardProvider {
    async fn get_text(&self) -> Result<String, ClipboardError> {
        Ok(self.content.read().await.clone())
    }

    async fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        *self.content.write().await = text.to_string();
        *self.change_count.write().await += 1;
        Ok(())
    }

    async fn get_formats(&self) -> Result<Vec<String>, ClipboardError> {
        Ok(vec!["text/plain".to_string()])
    }

    async fn has_format(&self, format: &str) -> Result<bool, ClipboardError> {
        Ok(format == "text/plain")
    }
}

async fn create_complete_setup() -> Result<(
    Arc<Config>,
    Arc<TestClipboardProvider>,
    Arc<HistoryManager>,
    Arc<PeerDiscovery>,
    Arc<TransportManager>,
    Arc<SyncEngine>,
)> {
    let temp_dir = TempDir::new()?;
    let config = Arc::new(Config::default_with_path(temp_dir.path().join("config.toml")));
    let clipboard = Arc::new(TestClipboardProvider::new());
    let history = Arc::new(HistoryManager::new(&temp_dir.path().join("history.db")).await?);
    let discovery = Arc::new(PeerDiscovery::new(config.clone()).await?);
    let transport = Arc::new(TransportManager::new(TransportConfig::default()));

    let sync_engine = Arc::new(SyncEngine::new(
        config.clone(),
        clipboard.clone(),
        history.clone(),
        discovery.clone(),
        transport.clone(),
    ));

    Ok((config, clipboard, history, discovery, transport, sync_engine))
}

#[tokio::test]
async fn test_complete_system_initialization() -> Result<()> {
    let (config, clipboard, history, discovery, transport, sync_engine) = 
        create_complete_setup().await?;

    // Test that all components can be initialized together
    assert!(!config.node_id.is_nil());
    assert_eq!(clipboard.get_text().await?, "");
    assert!(history.get_recent_entries(1).await?.is_empty());
    assert!(sync_engine.get_connected_peers().await.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_clipboard_to_history_flow() -> Result<()> {
    let (_config, clipboard, history, _discovery, _transport, sync_engine) = 
        create_complete_setup().await?;

    // Set clipboard content
    clipboard.set_text("Test content for history").await?;

    // Force sync to trigger history storage
    sync_engine.force_sync().await?;

    // Wait a bit for async operations
    sleep(Duration::from_millis(100)).await;

    // Check that content was saved to history
    let entries = history.get_recent_entries(5).await?;
    assert!(!entries.is_empty());
    
    match &entries[0].content {
        ClipboardData::Text(text) => {
            assert_eq!(text, "Test content for history");
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_sync_event_propagation() -> Result<()> {
    let (_config, clipboard, _history, _discovery, _transport, sync_engine) = 
        create_complete_setup().await?;

    // Subscribe to sync events
    let mut event_receiver = sync_engine.subscribe();

    // Trigger clipboard change
    clipboard.set_text("Event propagation test").await?;
    sync_engine.force_sync().await?;

    // Wait for event
    let event = timeout(Duration::from_secs(2), event_receiver.recv()).await??;
    
    match &event.entry.content {
        ClipboardData::Text(text) => {
            assert_eq!(text, "Event propagation test");
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_cli_integration_with_backend() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let config_path = temp_dir.path().join("config.toml");
    
    // Create config
    let config = Config::default_with_path(config_path.clone());
    config.save().await?;

    // Create CLI handler
    let mut cli_handler = CliHandler::new(Some(config_path)).await?;

    // Test various CLI commands
    cli_handler.handle_command(Commands::Status).await?;
    cli_handler.handle_command(Commands::History { limit: 10, interactive: false }).await?;
    cli_handler.handle_command(Commands::Peers).await?;

    // Copy and paste operations
    cli_handler.handle_command(Commands::Copy {
        text: "CLI integration test".to_string(),
    }).await?;

    cli_handler.handle_command(Commands::Paste).await?;

    Ok(())
}

#[tokio::test]
async fn test_multiple_clipboard_changes() -> Result<()> {
    let (_config, clipboard, history, _discovery, _transport, sync_engine) = 
        create_complete_setup().await?;

    // Make multiple clipboard changes
    let test_contents = vec![
        "First clipboard content",
        "Second clipboard content", 
        "Third clipboard content",
        "Fourth clipboard content",
        "Fifth clipboard content",
    ];

    for content in &test_contents {
        clipboard.set_text(content).await?;
        sync_engine.force_sync().await?;
        sleep(Duration::from_millis(50)).await; // Small delay between changes
    }

    // Check that all changes were recorded
    let entries = history.get_recent_entries(10).await?;
    assert_eq!(entries.len(), test_contents.len());

    // Verify order (most recent first)
    for (i, entry) in entries.iter().enumerate() {
        match &entry.content {
            ClipboardData::Text(text) => {
                let expected_idx = test_contents.len() - 1 - i;
                assert_eq!(text, test_contents[expected_idx]);
            }
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_large_content_handling() -> Result<()> {
    let (_config, clipboard, history, _discovery, _transport, sync_engine) = 
        create_complete_setup().await?;

    // Test with large content (100KB)
    let large_content = "Large content test. ".repeat(5000);
    clipboard.set_text(&large_content).await?;
    sync_engine.force_sync().await?;

    // Wait for processing
    sleep(Duration::from_millis(200)).await;

    // Verify large content was handled correctly
    let entries = history.get_recent_entries(1).await?;
    assert_eq!(entries.len(), 1);
    
    match &entries[0].content {
        ClipboardData::Text(text) => {
            assert_eq!(text.len(), large_content.len());
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_concurrent_operations() -> Result<()> {
    let (_config, clipboard, history, _discovery, _transport, sync_engine) = 
        create_complete_setup().await?;

    let sync_engine = Arc::new(sync_engine.as_ref());
    let clipboard = Arc::new(clipboard.as_ref());

    // Launch concurrent operations
    let mut handles = Vec::new();

    // Concurrent clipboard updates
    for i in 0..5 {
        let clipboard = clipboard.clone();
        let sync_engine = sync_engine.clone();
        
        let handle = tokio::spawn(async move {
            let content = format!("Concurrent content {}", i);
            clipboard.set_text(&content).await.unwrap();
            sync_engine.force_sync().await.unwrap();
        });
        
        handles.push(handle);
    }

    // Concurrent history reads
    for _ in 0..3 {
        let history = history.clone();
        
        let handle = tokio::spawn(async move {
            let _entries = history.get_recent_entries(10).await.unwrap();
        });
        
        handles.push(handle);
    }

    // Wait for all operations
    for handle in handles {
        handle.await?;
    }

    // System should still be functional
    let entries = history.get_recent_entries(10).await?;
    assert!(!entries.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_system_state_consistency() -> Result<()> {
    let (_config, clipboard, history, _discovery, _transport, sync_engine) = 
        create_complete_setup().await?;

    // Set initial content
    clipboard.set_text("Initial state").await?;
    sync_engine.force_sync().await?;

    // Get baseline state
    let initial_change_count = clipboard.get_change_count().await;
    let initial_history_count = history.get_recent_entries(100).await?.len();

    // Make changes
    clipboard.set_text("Changed state").await?;
    sync_engine.force_sync().await?;

    sleep(Duration::from_millis(100)).await;

    // Verify state changes
    let final_change_count = clipboard.get_change_count().await;
    let final_history_count = history.get_recent_entries(100).await?.len();

    assert!(final_change_count > initial_change_count);
    assert!(final_history_count > initial_history_count);

    // Verify current clipboard content matches latest history entry
    let current_content = clipboard.get_text().await?;
    let recent_entries = history.get_recent_entries(1).await?;
    
    if let Some(entry) = recent_entries.first() {
        match &entry.content {
            ClipboardData::Text(text) => {
                assert_eq!(current_content, *text);
            }
        }
    }

    Ok(())
}