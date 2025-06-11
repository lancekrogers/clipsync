use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tempfile::TempDir;
use tokio::time::timeout;

use clipsync::{
    config::Config,
    clipboard::{ClipboardProvider, ClipboardData, ClipboardError},
    history::HistoryManager,
    hotkey::{HotKeyManager, HotKeyAction, HotKeyEvent},
};

// Mock clipboard provider for testing
struct MockClipboardProvider {
    content: tokio::sync::RwLock<String>,
}

impl MockClipboardProvider {
    fn new() -> Self {
        Self {
            content: tokio::sync::RwLock::new("Initial content".to_string()),
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
)> {
    let temp_dir = TempDir::new()?;
    let config = Arc::new(Config::default_with_path(temp_dir.path().join("config.toml")));
    let clipboard = Arc::new(MockClipboardProvider::new());
    let history = Arc::new(HistoryManager::new(&temp_dir.path().join("history.db")).await?);

    Ok((config, clipboard, history))
}

#[tokio::test]
async fn test_hotkey_manager_creation() -> Result<()> {
    let (config, clipboard, history) = create_test_setup().await?;

    let hotkey_manager = HotKeyManager::new(config, clipboard, history)?;
    
    // Manager should be created successfully
    // Note: We can't test actual hotkey registration in headless environment
    
    Ok(())
}

#[tokio::test]
async fn test_hotkey_event_subscription() -> Result<()> {
    let (config, clipboard, history) = create_test_setup().await?;

    let hotkey_manager = HotKeyManager::new(config, clipboard, history)?;
    let _receiver = hotkey_manager.subscribe();
    
    // Subscription should work without errors
    Ok(())
}

#[tokio::test]
async fn test_hotkey_actions() -> Result<()> {
    // Test that hotkey actions can be cloned and compared
    let action1 = HotKeyAction::ShowHistory;
    let action2 = action1.clone();
    
    assert!(matches!(action2, HotKeyAction::ShowHistory));
    
    let action3 = HotKeyAction::ForceSync;
    assert!(matches!(action3, HotKeyAction::ForceSync));
    
    Ok(())
}

#[tokio::test]
async fn test_hotkey_event_structure() -> Result<()> {
    let event = HotKeyEvent {
        action: HotKeyAction::ShowHistory,
        hotkey_id: 123,
    };
    
    assert!(matches!(event.action, HotKeyAction::ShowHistory));
    assert_eq!(event.hotkey_id, 123);
    
    Ok(())
}

// Note: Most hotkey functionality tests require a GUI environment
// and actual keyboard events, which are not available in CI/headless tests.
// In a real test suite, you would:
// 1. Mock the global hotkey system
// 2. Simulate hotkey events
// 3. Test the action execution logic
// 4. Verify the correct responses to different hotkey combinations

#[tokio::test]
async fn test_hotkey_manager_unregister() -> Result<()> {
    let (config, clipboard, history) = create_test_setup().await?;

    let mut hotkey_manager = HotKeyManager::new(config, clipboard, history)?;
    
    // Test unregister all (should not panic even if no hotkeys registered)
    let result = hotkey_manager.unregister_all();
    assert!(result.is_ok());
    
    Ok(())
}

#[cfg(not(target_os = "linux"))] // Skip on Linux in CI where GUI is not available
#[tokio::test]
async fn test_default_hotkey_registration() -> Result<()> {
    let (config, clipboard, history) = create_test_setup().await?;

    let mut hotkey_manager = HotKeyManager::new(config, clipboard, history)?;
    
    // Try to register default hotkeys
    // This may fail in headless environment, but should not panic
    let _result = hotkey_manager.register_default_hotkeys().await;
    
    // Clean up
    let _ = hotkey_manager.unregister_all();
    
    Ok(())
}

#[tokio::test]
async fn test_multiple_hotkey_managers() -> Result<()> {
    // Test that multiple hotkey managers can be created
    // (though only one should be active at a time)
    
    let (config1, clipboard1, history1) = create_test_setup().await?;
    let (config2, clipboard2, history2) = create_test_setup().await?;

    let _manager1 = HotKeyManager::new(config1, clipboard1, history1)?;
    let _manager2 = HotKeyManager::new(config2, clipboard2, history2)?;
    
    // Both should be created successfully
    Ok(())
}

#[tokio::test]
async fn test_hotkey_manager_with_sync_engine() -> Result<()> {
    use clipsync::{
        discovery::PeerDiscovery,
        sync::SyncEngine,
        transport::{TransportManager, TransportConfig},
    };
    
    let (config, clipboard, history) = create_test_setup().await?;
    let discovery = Arc::new(PeerDiscovery::new(config.clone()).await?);
    let transport = Arc::new(TransportManager::new(TransportConfig::default()));
    
    let sync_engine = Arc::new(SyncEngine::new(
        config.clone(),
        clipboard.clone(),
        history.clone(),
        discovery,
        transport,
    ));

    let mut hotkey_manager = HotKeyManager::new(config, clipboard, history)?;
    hotkey_manager.set_sync_engine(sync_engine);
    
    // Should work without errors
    Ok(())
}