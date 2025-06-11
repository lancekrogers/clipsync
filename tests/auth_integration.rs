//! Integration tests for SSH authentication module

use clipsync::auth::*;
use tempfile::TempDir;

#[tokio::test]
async fn test_full_authentication_flow() {
    // Create temporary directories for testing
    let temp_dir = TempDir::new().unwrap();
    let key_path = temp_dir.path().join("test_key");
    let auth_keys_path = temp_dir.path().join("authorized_keys");
    
    // Create authentication config
    let config = AuthConfig {
        private_key_path: key_path.clone(),
        authorized_keys_path: auth_keys_path.clone(),
        generate_if_missing: true,
    };
    
    // Create first authenticator (peer A)
    let auth_a = SshAuthenticator::new(config.clone()).await.unwrap();
    let pubkey_a = auth_a.get_public_key().await.unwrap();
    
    // Create second key pair for peer B
    let keypair_b = KeyPair::generate(KeyType::Ed25519).unwrap();
    let pubkey_b = keypair_b.public_key();
    
    // Add peer B to authorized keys
    {
        let mut auth_keys = AuthorizedKeys::new();
        auth_keys.add_key(AuthorizedKey {
            public_key: pubkey_b.clone(),
            comment: Some("peer_b".to_string()),
            options: vec![],
        });
        auth_keys.save_to_file(&auth_keys_path).await.unwrap();
    }
    
    // Create new authenticator to reload authorized keys
    let auth_a = SshAuthenticator::new(config).await.unwrap();
    
    // Test peer B is authorized
    assert!(auth_a.is_authorized(&pubkey_b).await.unwrap());
    
    // Test authentication token generation
    let token = auth_a.authenticate_peer(&pubkey_b).await.unwrap();
    assert!(!token.token_id.is_empty());
    assert_eq!(token.peer_fingerprint, pubkey_b.fingerprint());
    assert!(!token.is_expired());
    
    // Test token verification
    let peer_id = auth_a.verify_token(&token).await.unwrap();
    assert_eq!(peer_id.fingerprint, pubkey_b.fingerprint());
    assert_eq!(peer_id.name.as_deref(), Some("peer_b"));
    
    // Test unauthorized peer is rejected
    let unauthorized_key = KeyPair::generate(KeyType::Ed25519).unwrap().public_key();
    let result = auth_a.authenticate_peer(&unauthorized_key).await;
    assert!(matches!(result, Err(AuthError::UnauthorizedPeer(_))));
}

#[tokio::test]
async fn test_key_generation_and_persistence() {
    let temp_dir = TempDir::new().unwrap();
    let key_path = temp_dir.path().join("persistent_key");
    
    // Generate initial key
    let original_key = KeyPair::generate(KeyType::Ed25519).unwrap();
    let original_fingerprint = original_key.public_key().fingerprint();
    original_key.save_to_file(&key_path).await.unwrap();
    
    // Load key back
    let loaded_key = KeyPair::load_from_file(&key_path).await.unwrap();
    let loaded_fingerprint = loaded_key.public_key().fingerprint();
    
    // Verify they match
    assert_eq!(original_fingerprint, loaded_fingerprint);
    
    // Verify public key file was created
    let pub_key_path = key_path.with_extension("pub");
    assert!(pub_key_path.exists());
    
    // Verify public key content
    let pub_content = tokio::fs::read_to_string(&pub_key_path).await.unwrap();
    assert!(pub_content.contains("ssh-ed25519"));
    assert!(pub_content.contains("clipsync@localhost"));
}

#[test]
fn test_openssh_key_format() {
    // Generate a key pair
    let key_pair = KeyPair::generate(KeyType::Ed25519).unwrap();
    let public_key = key_pair.public_key();
    
    // Convert to OpenSSH format
    let openssh_format = public_key.to_openssh();
    assert!(openssh_format.starts_with("ssh-ed25519 "));
    
    // Parse it back
    let parsed_key = PublicKey::from_openssh(&openssh_format).unwrap();
    assert_eq!(parsed_key.key_type, KeyType::Ed25519);
    assert_eq!(parsed_key.fingerprint(), public_key.fingerprint());
}

#[test]
fn test_signature_verification() {
    let key_pair = KeyPair::generate(KeyType::Ed25519).unwrap();
    let public_key = key_pair.public_key();
    
    let message = b"Hello, ClipSync authentication!";
    
    // Sign the message
    let signature = key_pair.sign(message).unwrap();
    
    // Verify with correct key
    assert!(public_key.verify(message, &signature).unwrap());
    
    // Verify fails with wrong message
    let wrong_message = b"Wrong message";
    assert!(!public_key.verify(wrong_message, &signature).unwrap());
    
    // Verify fails with different key
    let other_key = KeyPair::generate(KeyType::Ed25519).unwrap().public_key();
    assert!(!other_key.verify(message, &signature).unwrap());
}

#[tokio::test]
async fn test_authorized_keys_management() {
    let temp_dir = TempDir::new().unwrap();
    let auth_file = temp_dir.path().join("test_authorized_keys");
    
    // Create test keys
    let key1 = KeyPair::generate(KeyType::Ed25519).unwrap().public_key();
    let key2 = KeyPair::generate(KeyType::Ed25519).unwrap().public_key();
    
    // Create authorized keys
    let mut auth_keys = AuthorizedKeys::new();
    auth_keys.add_key(AuthorizedKey {
        public_key: key1.clone(),
        comment: Some("test-key-1".to_string()),
        options: vec!["no-port-forwarding".to_string()],
    });
    auth_keys.add_key(AuthorizedKey {
        public_key: key2.clone(),
        comment: Some("test-key-2".to_string()),
        options: vec![],
    });
    
    // Save to file
    auth_keys.save_to_file(&auth_file).await.unwrap();
    assert!(auth_file.exists());
    
    // Load from file
    let loaded_keys = AuthorizedKeys::load_from_file(&auth_file).await.unwrap();
    assert_eq!(loaded_keys.len(), 2);
    assert!(loaded_keys.is_authorized(&key1));
    assert!(loaded_keys.is_authorized(&key2));
    
    // Test key lookup
    let found_key = loaded_keys.get_key_by_fingerprint(&key1.fingerprint()).unwrap();
    assert_eq!(found_key.comment.as_deref(), Some("test-key-1"));
    assert_eq!(found_key.options, vec!["no-port-forwarding"]);
}

#[tokio::test]
async fn test_authenticator_error_cases() {
    let temp_dir = TempDir::new().unwrap();
    
    // Test with missing key file (generate_if_missing = false)
    let config = AuthConfig {
        private_key_path: temp_dir.path().join("nonexistent_key"),
        authorized_keys_path: temp_dir.path().join("authorized_keys"),
        generate_if_missing: false,
    };
    
    let auth = SshAuthenticator::new(config).await.unwrap();
    
    // Should fail to get public key when no key exists
    let result = auth.get_public_key().await;
    assert!(result.is_err());
}

#[test]
fn test_token_expiration() {
    use std::time::{SystemTime, UNIX_EPOCH};
    
    // Create an expired token
    let mut token = AuthToken {
        token_id: "test_token".to_string(),
        peer_fingerprint: "test_fingerprint".to_string(),
        created_at: 1000000000, // Old timestamp
        expires_at: 1000000001,  // Even older expiration
        signature: vec![],
    };
    
    assert!(token.is_expired());
    
    // Create a non-expired token
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    token.expires_at = now + 3600; // 1 hour from now
    assert!(!token.is_expired());
}