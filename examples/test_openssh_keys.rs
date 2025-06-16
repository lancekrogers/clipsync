//! Example demonstrating OpenSSH key support in ClipSync
//!
//! This example shows how to work with both PKCS8 and OpenSSH format keys.

use clipsync::auth::{KeyPair, KeyType};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("ClipSync SSH Key Format Support Demo\n");
    
    // Generate a new Ed25519 key
    println!("1. Generating new Ed25519 key pair...");
    let key_pair = KeyPair::generate(KeyType::Ed25519)?;
    println!("   ✓ Generated Ed25519 key");
    println!("   Fingerprint: {}", key_pair.public_key().fingerprint());
    
    // Save to file (PKCS8 format)
    let temp_dir = tempfile::TempDir::new()?;
    let key_path = temp_dir.path().join("clipsync_key");
    
    println!("\n2. Saving key to file (PKCS8 format)...");
    key_pair.save_to_file(&key_path).await?;
    println!("   ✓ Saved private key to: {}", key_path.display());
    println!("   ✓ Saved public key to: {}.pub", key_path.display());
    
    // Load the key back
    println!("\n3. Loading key from file...");
    let loaded_key = KeyPair::load_from_file(&key_path).await?;
    println!("   ✓ Loaded key successfully");
    println!("   Fingerprint: {}", loaded_key.public_key().fingerprint());
    
    // Test with OpenSSH format
    println!("\n4. OpenSSH Format Support:");
    println!("   - Unencrypted Ed25519 keys: Partial support");
    println!("   - Encrypted keys: Not supported (decrypt with ssh-keygen -p -N '')");
    println!("   - RSA keys: Convert to PKCS8 (ssh-keygen -p -m PKCS8)");
    println!("   - EC keys: Not supported");
    
    // Example OpenSSH key (this will fail with current implementation)
    let openssh_example = r#"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACBd6I/VcoEWJAfCYmVGLfPTn9cC6vr+bfTmpGNrAQwEhgAAAJgAeDRhAHg0
YQAAAAtzc2gtZWQyNTUxOQAAACBd6I/VcoEWJAfCYmVGLfPTn9cC6vr+bfTmpGNrAQwEhg
AAAECJ0/JvHlNpZLQs3P5STDPSDKXOqvDcUvWO1Kv1nkMUKl3oj9VygRYkB8JiZUYt89Of
1wLq+v5t9OakY2sBDASGAAAAFXRlc3RAY2xpcHN5bmMubG9jYWwBAg==
-----END OPENSSH PRIVATE KEY-----"#;
    
    println!("\n5. Testing OpenSSH format parsing...");
    match KeyPair::from_private_key_bytes(openssh_example.as_bytes()) {
        Ok(_) => println!("   ✓ Successfully parsed OpenSSH format key"),
        Err(e) => println!("   ℹ OpenSSH format note: {}", e),
    }
    
    // Demonstrate signing and verification
    println!("\n6. Testing cryptographic operations...");
    let message = b"Hello, ClipSync!";
    let signature = key_pair.sign(message)?;
    println!("   ✓ Signed message");
    
    let verified = key_pair.public_key().verify(message, &signature)?;
    println!("   ✓ Signature verified: {}", verified);
    
    println!("\n✨ Demo completed successfully!");
    
    Ok(())
}