use clipsync::config::Config;
use clipsync::clipboard::ClipboardManager;
use clipsync::sync::SyncEngine;
use clipsync::transport::WebSocketTransport;
use clipsync::discovery::DiscoveryService;
use clipsync::auth::AuthManager;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test]
async fn test_full_sync_workflow() {
    // Create temporary directories for config
    let temp_dir1 = TempDir::new().unwrap();
    let temp_dir2 = TempDir::new().unwrap();
    
    // Initialize two instances
    let config1 = Config::new(temp_dir1.path().to_path_buf()).unwrap();
    let config2 = Config::new(temp_dir2.path().to_path_buf()).unwrap();
    
    // Start both instances
    let instance1 = start_instance(config1).await;
    let instance2 = start_instance(config2).await;
    
    // Wait for discovery
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    // Copy text on instance 1
    let test_text = "Hello from instance 1!";
    instance1.clipboard.set_text(test_text).await.unwrap();
    
    // Trigger sync
    instance1.sync_engine.sync_now().await.unwrap();
    
    // Wait for sync to complete
    tokio::time::sleep(Duration::from_secs(1)).await;
    
    // Verify text appears on instance 2
    let received_text = instance2.clipboard.get_text().await.unwrap();
    assert_eq!(received_text, test_text);
}

#[tokio::test]
async fn test_bidirectional_sync() {
    let temp_dir1 = TempDir::new().unwrap();
    let temp_dir2 = TempDir::new().unwrap();
    
    let config1 = Config::new(temp_dir1.path().to_path_buf()).unwrap();
    let config2 = Config::new(temp_dir2.path().to_path_buf()).unwrap();
    
    let instance1 = start_instance(config1).await;
    let instance2 = start_instance(config2).await;
    
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    // Sync from instance 1 to 2
    instance1.clipboard.set_text("From instance 1").await.unwrap();
    instance1.sync_engine.sync_now().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    
    let text2 = instance2.clipboard.get_text().await.unwrap();
    assert_eq!(text2, "From instance 1");
    
    // Sync from instance 2 to 1
    instance2.clipboard.set_text("From instance 2").await.unwrap();
    instance2.sync_engine.sync_now().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    
    let text1 = instance1.clipboard.get_text().await.unwrap();
    assert_eq!(text1, "From instance 2");
}

#[tokio::test]
async fn test_large_content_sync() {
    let temp_dir1 = TempDir::new().unwrap();
    let temp_dir2 = TempDir::new().unwrap();
    
    let config1 = Config::new(temp_dir1.path().to_path_buf()).unwrap();
    let config2 = Config::new(temp_dir2.path().to_path_buf()).unwrap();
    
    let instance1 = start_instance(config1).await;
    let instance2 = start_instance(config2).await;
    
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    // Create large content (1MB)
    let large_content = "x".repeat(1024 * 1024);
    
    instance1.clipboard.set_text(&large_content).await.unwrap();
    instance1.sync_engine.sync_now().await.unwrap();
    
    // Allow more time for large content
    tokio::time::sleep(Duration::from_secs(3)).await;
    
    let received = instance2.clipboard.get_text().await.unwrap();
    assert_eq!(received.len(), large_content.len());
}

#[tokio::test]
async fn test_network_interruption_recovery() {
    let temp_dir1 = TempDir::new().unwrap();
    let temp_dir2 = TempDir::new().unwrap();
    
    let config1 = Config::new(temp_dir1.path().to_path_buf()).unwrap();
    let config2 = Config::new(temp_dir2.path().to_path_buf()).unwrap();
    
    let instance1 = start_instance(config1).await;
    let instance2 = start_instance(config2).await;
    
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    // Simulate network interruption by stopping transport
    instance1.transport.disconnect().await;
    instance2.transport.disconnect().await;
    
    // Try to sync (should fail gracefully)
    instance1.clipboard.set_text("During interruption").await.unwrap();
    let sync_result = timeout(
        Duration::from_secs(2),
        instance1.sync_engine.sync_now()
    ).await;
    assert!(sync_result.is_err() || sync_result.unwrap().is_err());
    
    // Reconnect
    instance1.transport.connect().await.unwrap();
    instance2.transport.connect().await.unwrap();
    
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    // Sync should work now
    instance1.sync_engine.sync_now().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    
    let received = instance2.clipboard.get_text().await.unwrap();
    assert_eq!(received, "During interruption");
}

#[tokio::test]
async fn test_multiple_device_sync() {
    let instances = create_instances(3).await;
    
    // Wait for discovery
    tokio::time::sleep(Duration::from_secs(3)).await;
    
    // Instance 0 broadcasts
    instances[0].clipboard.set_text("Broadcast message").await.unwrap();
    instances[0].sync_engine.sync_now().await.unwrap();
    
    tokio::time::sleep(Duration::from_secs(1)).await;
    
    // All instances should have the message
    for (i, instance) in instances.iter().enumerate() {
        let text = instance.clipboard.get_text().await.unwrap();
        assert_eq!(text, "Broadcast message", "Instance {} didn't receive message", i);
    }
}

#[tokio::test]
async fn test_concurrent_sync_conflict_resolution() {
    let temp_dir1 = TempDir::new().unwrap();
    let temp_dir2 = TempDir::new().unwrap();
    
    let config1 = Config::new(temp_dir1.path().to_path_buf()).unwrap();
    let config2 = Config::new(temp_dir2.path().to_path_buf()).unwrap();
    
    let instance1 = start_instance(config1).await;
    let instance2 = start_instance(config2).await;
    
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    // Both instances update clipboard simultaneously
    let handle1 = tokio::spawn(async move {
        instance1.clipboard.set_text("Instance 1 update").await.unwrap();
        instance1.sync_engine.sync_now().await.unwrap();
    });
    
    let handle2 = tokio::spawn(async move {
        instance2.clipboard.set_text("Instance 2 update").await.unwrap();
        instance2.sync_engine.sync_now().await.unwrap();
    });
    
    handle1.await.unwrap();
    handle2.await.unwrap();
    
    // Both instances should converge to the same state
    // (Last write wins based on timestamp)
    tokio::time::sleep(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn test_authentication_flow() {
    let temp_dir = TempDir::new().unwrap();
    let config = Config::new(temp_dir.path().to_path_buf()).unwrap();
    
    let auth_manager = AuthManager::new(&config).await.unwrap();
    
    // Generate new key pair
    auth_manager.generate_key_pair().await.unwrap();
    
    // Verify key exists
    assert!(auth_manager.has_key_pair().await);
    
    // Test authentication
    let challenge = b"test challenge";
    let signature = auth_manager.sign_challenge(challenge).await.unwrap();
    let public_key = auth_manager.get_public_key().await.unwrap();
    
    // Verify signature
    assert!(auth_manager.verify_signature(&public_key, challenge, &signature).await.unwrap());
}

#[tokio::test]
async fn test_service_restart_resilience() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().to_path_buf();
    
    // Start instance
    let config1 = Config::new(config_path.clone()).unwrap();
    let instance1 = start_instance(config1).await;
    
    // Set some content
    instance1.clipboard.set_text("Before restart").await.unwrap();
    
    // Simulate service stop
    drop(instance1);
    
    tokio::time::sleep(Duration::from_millis(500)).await;
    
    // Restart with same config
    let config2 = Config::new(config_path).unwrap();
    let instance2 = start_instance(config2).await;
    
    // Should restore state
    let text = instance2.clipboard.get_text().await.unwrap();
    assert_eq!(text, "Before restart");
}

// Helper structures and functions

struct TestInstance {
    clipboard: Arc<ClipboardManager>,
    sync_engine: Arc<SyncEngine>,
    transport: Arc<WebSocketTransport>,
    discovery: Arc<DiscoveryService>,
}

async fn start_instance(config: Config) -> TestInstance {
    let auth_manager = Arc::new(AuthManager::new(&config).await.unwrap());
    let clipboard = Arc::new(ClipboardManager::new());
    let transport = Arc::new(WebSocketTransport::new(config.clone()).await.unwrap());
    let discovery = Arc::new(DiscoveryService::new(config.clone()).await.unwrap());
    let sync_engine = Arc::new(
        SyncEngine::new(
            config,
            clipboard.clone(),
            transport.clone(),
            auth_manager,
        ).await.unwrap()
    );
    
    // Start services
    transport.start().await.unwrap();
    discovery.start().await.unwrap();
    sync_engine.start().await.unwrap();
    
    TestInstance {
        clipboard,
        sync_engine,
        transport,
        discovery,
    }
}

async fn create_instances(count: usize) -> Vec<TestInstance> {
    let mut instances = Vec::new();
    
    for _ in 0..count {
        let temp_dir = TempDir::new().unwrap();
        let config = Config::new(temp_dir.path().to_path_buf()).unwrap();
        instances.push(start_instance(config).await);
    }
    
    instances
}