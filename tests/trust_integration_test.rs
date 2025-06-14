//! Integration tests for the trust system

use clipsync::auth::{KeyPair, KeyType, TrustDecision, TrustManager};
use clipsync::discovery::{PeerInfo, PeerMetadata};
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;

#[tokio::test]
async fn test_trust_on_first_use() {
    let temp_dir = TempDir::new().unwrap();

    // Create trust manager that auto-trusts for testing
    let trust_manager = Arc::new(
        TrustManager::with_prompt_callback(temp_dir.path().to_path_buf(), |peer, fingerprint| {
            println!(
                "Test: Would prompt for peer {} with fingerprint {}",
                peer.name, fingerprint
            );
            TrustDecision::Trust
        })
        .unwrap(),
    );

    // Generate a test key
    let key_pair = KeyPair::generate(KeyType::Ed25519).unwrap();
    let public_key = key_pair.public_key();
    let openssh_key = public_key.to_openssh();

    // Create a test peer with public key
    let peer_info = PeerInfo {
        id: Uuid::new_v4(),
        name: "test-device".to_string(),
        addresses: vec!["127.0.0.1:8484".parse().unwrap()],
        port: 8484,
        version: "1.0.0".to_string(),
        platform: "test".to_string(),
        metadata: PeerMetadata {
            ssh_public_key: Some(openssh_key),
            ssh_fingerprint: Some(public_key.fingerprint()),
            capabilities: vec![],
            device_name: Some("Test Device".to_string()),
        },
        last_seen: chrono::Utc::now().timestamp(),
    };

    // Process the peer
    let trusted = trust_manager
        .process_peer(&peer_info, &public_key)
        .await
        .unwrap();
    assert!(trusted);

    // Verify it's now trusted
    assert!(trust_manager.is_trusted(&public_key.fingerprint()).await);

    // Verify persistence
    trust_manager.load().await.unwrap();
    assert!(trust_manager.is_trusted(&public_key.fingerprint()).await);
}

#[tokio::test]
async fn test_reject_peer() {
    let temp_dir = TempDir::new().unwrap();

    // Create trust manager that rejects for testing
    let trust_manager = Arc::new(
        TrustManager::with_prompt_callback(temp_dir.path().to_path_buf(), |_, _| {
            TrustDecision::Reject
        })
        .unwrap(),
    );

    // Generate a test key
    let key_pair = KeyPair::generate(KeyType::Ed25519).unwrap();
    let public_key = key_pair.public_key();
    let openssh_key = public_key.to_openssh();

    // Create a test peer with public key
    let peer_info = PeerInfo {
        id: Uuid::new_v4(),
        name: "untrusted-device".to_string(),
        addresses: vec!["127.0.0.1:8484".parse().unwrap()],
        port: 8484,
        version: "1.0.0".to_string(),
        platform: "test".to_string(),
        metadata: PeerMetadata {
            ssh_public_key: Some(openssh_key),
            ssh_fingerprint: Some(public_key.fingerprint()),
            capabilities: vec![],
            device_name: Some("Untrusted Device".to_string()),
        },
        last_seen: chrono::Utc::now().timestamp(),
    };

    // Process the peer
    let trusted = trust_manager
        .process_peer(&peer_info, &public_key)
        .await
        .unwrap();
    assert!(!trusted);

    // Verify it's not trusted
    assert!(!trust_manager.is_trusted(&public_key.fingerprint()).await);
}
