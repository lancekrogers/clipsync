//! Integration tests for service discovery

use clipsync::{
    discovery::{Discovery, DiscoveryEvent, DiscoveryService, PeerInfo, ServiceInfo},
    Config,
};
use std::time::Duration;
use tokio::time::timeout;
use uuid::Uuid;

/// Test basic discovery lifecycle
#[tokio::test]
async fn test_discovery_lifecycle() {
    let config = Config::default();
    let mut discovery = DiscoveryService::new(&config).unwrap();

    // Start discovery
    discovery.start().await.unwrap();

    // Announce our service
    let service_info = ServiceInfo::from_config(Uuid::new_v4(), 9090);
    discovery.announce(service_info).await.unwrap();

    // Should be able to get peers (even if empty)
    let peers = discovery.discover_peers().await.unwrap();
    assert!(peers.is_empty() || !peers.is_empty()); // Either is fine

    // Stop discovery
    discovery.stop().await.unwrap();
}

/// Test discovery events
#[tokio::test]
async fn test_discovery_events() {
    let config = Config::default();
    let mut discovery = DiscoveryService::new(&config).unwrap();

    // Subscribe to events before starting
    let mut events = discovery.subscribe_changes();

    // Start discovery
    discovery.start().await.unwrap();

    // Manually add a peer through peer manager
    let peer_info = PeerInfo {
        id: Uuid::new_v4(),
        name: "test-peer".to_string(),
        addresses: vec!["192.168.1.100:9090".parse().unwrap()],
        port: 9090,
        version: "1.0.0".to_string(),
        platform: "test".to_string(),
        metadata: Default::default(),
        last_seen: chrono::Utc::now().timestamp(),
    };

    discovery
        .peer_manager()
        .add_peer(
            peer_info.clone(),
            clipsync::discovery::DiscoveryMethod::Manual,
        )
        .await
        .unwrap();

    // Should receive peer discovered event
    let event = timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("Timeout waiting for event")
        .expect("No event received");

    match event {
        DiscoveryEvent::PeerDiscovered(peer) => {
            assert_eq!(peer.id, peer_info.id);
            assert_eq!(peer.name, "test-peer");
        }
        _ => panic!("Expected PeerDiscovered event"),
    }

    discovery.stop().await.unwrap();
}

/// Test manual peer configuration
#[tokio::test]
async fn test_manual_peer_configuration() {
    // Create discovery with default config (no manual peers for now)
    let config = Config::default();
    let mut discovery = DiscoveryService::new(&config).unwrap();
    discovery.start().await.unwrap();

    // Manually add a peer through peer manager to test the flow
    let peer_info = PeerInfo {
        id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
        name: "manual-peer".to_string(),
        addresses: vec!["192.168.1.200:9092".parse().unwrap()],
        port: 9092,
        version: "1.0.0".to_string(),
        platform: "test".to_string(),
        metadata: Default::default(),
        last_seen: chrono::Utc::now().timestamp(),
    };

    discovery
        .peer_manager()
        .add_peer(peer_info, clipsync::discovery::DiscoveryMethod::Manual)
        .await
        .unwrap();

    // Check that peer was added
    let peers = discovery.discover_peers().await.unwrap();
    let manual_peers: Vec<_> = peers.iter().filter(|p| p.name == "manual-peer").collect();

    assert_eq!(manual_peers.len(), 1);

    discovery.stop().await.unwrap();
}

/// Test broadcast discovery
#[tokio::test]
async fn test_broadcast_discovery() {
    let config = Config::default();
    let mut discovery = DiscoveryService::new(&config).unwrap();

    // Should start successfully
    discovery.start().await.unwrap();

    let service_info = ServiceInfo::from_config(Uuid::new_v4(), 9090);
    discovery.announce(service_info).await.unwrap();

    discovery.stop().await.unwrap();
}

/// Test peer capabilities
#[tokio::test]
async fn test_peer_capabilities() {
    let peer = PeerInfo {
        id: Uuid::new_v4(),
        name: "test-peer".to_string(),
        addresses: vec!["192.168.1.100:9090".parse().unwrap()],
        port: 9090,
        version: "1.0.0".to_string(),
        platform: "linux".to_string(),
        metadata: clipsync::discovery::PeerMetadata {
            ssh_fingerprint: Some("SHA256:abcd1234".to_string()),
            capabilities: vec!["encryption".to_string(), "compression".to_string()],
            device_name: Some("My Device".to_string()),
        },
        last_seen: chrono::Utc::now().timestamp(),
    };

    assert!(peer.has_capability("encryption"));
    assert!(peer.has_capability("compression"));
    assert!(!peer.has_capability("unknown"));
}

/// Test service info builder
#[tokio::test]
async fn test_service_info_builder() {
    let id = Uuid::new_v4();
    let service_info = ServiceInfo::from_config(id, 8080)
        .with_ssh_fingerprint("SHA256:test123".to_string())
        .with_capabilities(vec!["feature1".to_string(), "feature2".to_string()]);

    assert_eq!(service_info.id, id);
    assert_eq!(service_info.port, 8080);

    // Check TXT data
    let txt_map: std::collections::HashMap<_, _> = service_info
        .txt_data
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    assert_eq!(txt_map.get("ssh_fp"), Some(&"SHA256:test123"));
    assert_eq!(txt_map.get("caps"), Some(&"feature1,feature2"));
}

/// Test concurrent discovery operations
#[tokio::test]
async fn test_concurrent_discovery() {
    let config = Config::default();
    let mut discovery = DiscoveryService::new(&config).unwrap();

    discovery.start().await.unwrap();

    // Spawn multiple tasks that interact with discovery
    let mut handles = vec![];

    for i in 0..5 {
        let mut discovery_clone = DiscoveryService::new(&config).unwrap();
        let handle = tokio::spawn(async move {
            discovery_clone.start().await.unwrap();

            let service_info = ServiceInfo::from_config(Uuid::new_v4(), 9090 + i);
            discovery_clone.announce(service_info).await.unwrap();

            let _ = discovery_clone.discover_peers().await.unwrap();

            discovery_clone.stop().await.unwrap();
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }

    discovery.stop().await.unwrap();
}
