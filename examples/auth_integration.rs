//! Example showing how the transport layer would integrate with auth module

use clipsync::auth::*;
use clipsync::config::Config;
use std::sync::Arc;
use tempfile::TempDir;

/// Mock transport layer that uses authentication
struct MockTransport {
    authenticator: Arc<SshAuthenticator>,
    #[allow(dead_code)]
    config: Arc<Config>,
}

impl MockTransport {
    async fn new(config: Config) -> Result<Self, Box<dyn std::error::Error>> {
        // Create auth config from main config
        let auth_config = AuthConfig {
            private_key_path: config.auth.ssh_key.clone(),
            authorized_keys_path: config.auth.authorized_keys.clone(),
            generate_if_missing: true,
        };

        let authenticator = Arc::new(SshAuthenticator::new(auth_config).await?);

        Ok(Self {
            authenticator,
            config: Arc::new(config),
        })
    }

    /// Simulate handling an incoming connection
    async fn handle_incoming_connection(
        &self,
        peer_pubkey: &PublicKey,
    ) -> Result<String, Box<dyn std::error::Error>> {
        println!(
            "📞 Incoming connection from peer: {}",
            peer_pubkey.fingerprint()
        );

        // Check if peer is authorized
        if !self.authenticator.is_authorized(peer_pubkey).await? {
            println!("❌ Peer not authorized, rejecting connection");
            return Err("Unauthorized peer".into());
        }

        println!("✅ Peer is authorized, proceeding with authentication");

        // Generate authentication token
        let token = self.authenticator.authenticate_peer(peer_pubkey).await?;
        println!("🎫 Generated auth token: {}", token.token_id);

        // In real implementation, this token would be sent to the peer
        // and used for subsequent requests

        Ok(format!(
            "Connection established with peer {}",
            peer_pubkey.fingerprint()
        ))
    }

    /// Simulate validating a request with an auth token
    async fn validate_request(
        &self,
        token: &AuthToken,
    ) -> Result<PeerId, Box<dyn std::error::Error>> {
        println!("🔍 Validating request with token: {}", token.token_id);

        let peer_id = self.authenticator.verify_token(token).await?;
        println!("✅ Token valid for peer: {}", peer_id.fingerprint);

        Ok(peer_id)
    }

    /// Get our public key for advertising to peers
    async fn get_our_public_key(&self) -> Result<PublicKey, Box<dyn std::error::Error>> {
        Ok(self.authenticator.get_public_key().await?)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("ClipSync Auth Integration Demo");
    println!("=============================\n");

    // Create temporary directory for demo
    let temp_dir = TempDir::new()?;
    let demo_dir = temp_dir.path();

    // Create config
    let mut config = Config::default();
    config.auth.ssh_key = demo_dir.join("transport_key");
    config.auth.authorized_keys = demo_dir.join("authorized_keys");

    // Create the mock transport
    println!("🚀 Creating mock transport with authentication...");
    let transport = MockTransport::new(config).await?;

    let our_pubkey = transport.get_our_public_key().await?;
    println!("🔑 Our public key: {}\n", our_pubkey.fingerprint());

    // Add ourselves to authorized keys for demo
    {
        let mut auth_keys = AuthorizedKeys::new();
        auth_keys.add_key(AuthorizedKey {
            public_key: our_pubkey.clone(),
            comment: Some("self".to_string()),
            options: vec![],
        });

        // Add another peer
        let other_peer = KeyPair::generate(KeyType::Ed25519)?.public_key();
        auth_keys.add_key(AuthorizedKey {
            public_key: other_peer,
            comment: Some("trusted-peer".to_string()),
            options: vec!["no-port-forwarding".to_string()],
        });

        auth_keys
            .save_to_file(&demo_dir.join("authorized_keys"))
            .await?;
        println!("📋 Created authorized_keys with 2 peers\n");
    }

    // Create a new transport instance to reload authorized keys
    let transport = MockTransport::new(Config {
        auth: clipsync::config::AuthConfig {
            ssh_key: demo_dir.join("transport_key"),
            authorized_keys: demo_dir.join("authorized_keys"),
        },
        ..Config::default()
    })
    .await?;

    // Demo 1: Handle incoming connection from authorized peer
    println!("Demo 1: Authorized peer connection");
    println!("----------------------------------");

    let connection_result = transport.handle_incoming_connection(&our_pubkey).await?;
    println!("Result: {}\n", connection_result);

    // Demo 2: Handle incoming connection from unauthorized peer
    println!("Demo 2: Unauthorized peer connection");
    println!("------------------------------------");

    let unauthorized_key = KeyPair::generate(KeyType::Ed25519)?.public_key();
    match transport
        .handle_incoming_connection(&unauthorized_key)
        .await
    {
        Ok(_) => println!("❌ Unexpected success!"),
        Err(e) => println!("✅ Correctly rejected: {}\n", e),
    }

    // Demo 3: Token validation workflow
    println!("Demo 3: Token validation workflow");
    println!("---------------------------------");

    // First, authenticate to get a token
    let token = transport
        .authenticator
        .authenticate_peer(&our_pubkey)
        .await?;
    println!("🎫 Generated token for subsequent requests");

    // Now validate the token (simulating a later request)
    let peer_id = transport.validate_request(&token).await?;
    println!("✅ Request authorized for peer: {}", peer_id.fingerprint);
    if let Some(name) = peer_id.name {
        println!("   Peer name: {}", name);
    }

    // Demo 4: Show how agent coordination would work
    println!("\nDemo 4: Agent coordination pattern");
    println!("----------------------------------");

    println!("In the real ClipSync system:");
    println!("1. Agent 1 (Auth) provides the Authenticator trait");
    println!("2. Agent 2 (Transport) uses it for peer authentication");
    println!("3. Agent 3 (Sync Engine) gets authenticated peer info");
    println!("4. All components share the same auth configuration");

    println!("\nKey integration points:");
    println!("• Transport layer calls authenticator.is_authorized() for incoming connections");
    println!("• Transport layer calls authenticator.authenticate_peer() to generate tokens");
    println!("• Transport layer calls authenticator.verify_token() for each request");
    println!("• Sync engine trusts peer_id from verified tokens");

    println!("\nDemo completed successfully! 🎉");

    Ok(())
}
