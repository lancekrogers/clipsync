//! Demonstration of ClipSync SSH authentication

use clipsync::auth::*;
use std::path::PathBuf;
use tempfile::TempDir;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("ClipSync SSH Authentication Demo");
    println!("================================\n");

    // Create temporary directory for this demo
    let temp_dir = TempDir::new()?;
    let demo_dir = temp_dir.path();

    // 1. Generate SSH key pair
    println!("1. Generating SSH key pair...");
    let key_pair = KeyPair::generate(KeyType::Ed25519)?;
    let public_key = key_pair.public_key();

    println!("   Key type: {:?}", public_key.key_type);
    println!("   Fingerprint: {}", public_key.fingerprint());
    println!("   OpenSSH format: {}\n", public_key.to_openssh());

    // 2. Save key to file
    println!("2. Saving key pair to files...");
    let key_path = demo_dir.join("demo_key");
    key_pair.save_to_file(&key_path).await?;

    println!("   Private key saved to: {}", key_path.display());
    println!(
        "   Public key saved to: {}\n",
        key_path.with_extension("pub").display()
    );

    // 3. Create authorized keys file
    println!("3. Creating authorized keys file...");
    let auth_keys_path = demo_dir.join("authorized_keys");
    let mut auth_keys = AuthorizedKeys::new();

    // Add our own key (for demo purposes)
    auth_keys.add_key(AuthorizedKey {
        public_key: public_key.clone(),
        comment: Some("demo-key".to_string()),
        options: vec!["no-port-forwarding".to_string()],
    });

    // Add a second demo key
    let other_key = KeyPair::generate(KeyType::Ed25519)?;
    auth_keys.add_key(AuthorizedKey {
        public_key: other_key.public_key(),
        comment: Some("other-peer".to_string()),
        options: vec![],
    });

    auth_keys.save_to_file(&auth_keys_path).await?;
    println!("   Authorized keys saved to: {}", auth_keys_path.display());
    println!("   Added {} authorized keys\n", auth_keys.len());

    // 4. Create SSH authenticator
    println!("4. Creating SSH authenticator...");
    let auth_config = AuthConfig {
        private_key_path: key_path,
        authorized_keys_path: auth_keys_path,
        generate_if_missing: false,
    };

    let authenticator = SshAuthenticator::new(auth_config).await?;
    println!("   Authenticator created successfully");

    let local_pubkey = authenticator.get_public_key().await?;
    println!("   Local public key: {}\n", local_pubkey.fingerprint());

    // 5. Test authentication
    println!("5. Testing peer authentication...");

    // Test with authorized key
    if authenticator.is_authorized(&public_key).await? {
        println!("   ✓ Our key is authorized");

        let token = authenticator.authenticate_peer(&public_key).await?;
        println!("   ✓ Authentication token generated:");
        println!("     Token ID: {}", token.token_id);
        println!("     Peer fingerprint: {}", token.peer_fingerprint);
        println!("     Expires at: {}", token.expires_at);

        // Verify the token
        let peer_id = authenticator.verify_token(&token).await?;
        println!("   ✓ Token verification successful:");
        println!("     Peer ID: {}", peer_id.fingerprint);
        if let Some(name) = peer_id.name {
            println!("     Peer name: {}", name);
        }
    } else {
        println!("   ✗ Our key is not authorized (unexpected!)");
    }

    // Test with unauthorized key
    let unauthorized_key = KeyPair::generate(KeyType::Ed25519)?.public_key();
    println!(
        "\n   Testing unauthorized key: {}",
        unauthorized_key.fingerprint()
    );

    if authenticator.is_authorized(&unauthorized_key).await? {
        println!("   ✗ Unauthorized key was accepted (unexpected!)");
    } else {
        println!("   ✓ Unauthorized key was correctly rejected");

        match authenticator.authenticate_peer(&unauthorized_key).await {
            Ok(_) => println!("   ✗ Authentication succeeded (unexpected!)"),
            Err(AuthError::UnauthorizedPeer(_)) => {
                println!("   ✓ Authentication correctly failed with UnauthorizedPeer error");
            }
            Err(e) => println!("   ? Authentication failed with unexpected error: {}", e),
        }
    }

    // 6. Demonstrate signature verification
    println!("\n6. Testing digital signatures...");
    let message = b"Hello from ClipSync!";
    let signature = key_pair.sign(message)?;

    println!("   Message: {}", std::str::from_utf8(message)?);
    println!("   Signature length: {} bytes", signature.len());

    if public_key.verify(message, &signature)? {
        println!("   ✓ Signature verification successful");
    } else {
        println!("   ✗ Signature verification failed");
    }

    // Test with wrong message
    let wrong_message = b"Wrong message";
    if public_key.verify(wrong_message, &signature)? {
        println!("   ✗ Wrong message verified (unexpected!)");
    } else {
        println!("   ✓ Wrong message correctly rejected");
    }

    println!("\nDemo completed successfully!");
    println!(
        "Files created in temporary directory: {}",
        demo_dir.display()
    );

    Ok(())
}
