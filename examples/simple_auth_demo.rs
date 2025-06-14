//! Example demonstrating the simplified authentication flow

use clipsync::auth::{KeyPair, KeyType};
use clipsync::config::Config;
use clipsync::discovery::{PeerInfo, PeerMetadata};
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    println!("=== ClipSync Simplified Authentication Demo ===\n");

    // Load config (this would normally come from the actual config file)
    let config = Config::default();

    // 1. Show how the system auto-discovers the public key
    println!("1. Auto-discovering public key from private key path:");
    println!("   Private key: {}", config.auth.ssh_key.display());
    println!(
        "   Public key: {}",
        config.auth.get_public_key_path().display()
    );

    // 2. Generate a test key pair if needed
    let key_pair = KeyPair::generate(KeyType::Ed25519)?;
    let public_key = key_pair.public_key();
    let openssh_format = public_key.to_openssh();

    println!("\n2. Generated test SSH key:");
    println!("   Fingerprint: {}", public_key.fingerprint());
    println!("   OpenSSH format: {}...", &openssh_format[..50]);

    // 3. Simulate device discovery with public key in mDNS
    let peer_info = PeerInfo {
        id: Uuid::new_v4(),
        name: "laptop".to_string(),
        addresses: vec!["192.168.1.100:8484".parse().unwrap()],
        port: 8484,
        version: "1.0.0".to_string(),
        platform: "macos".to_string(),
        metadata: PeerMetadata {
            ssh_public_key: Some(openssh_format.clone()),
            ssh_fingerprint: Some(public_key.fingerprint()),
            capabilities: vec!["sync".to_string()],
            device_name: Some("My Laptop".to_string()),
        },
        last_seen: chrono::Utc::now().timestamp(),
    };

    println!("\n3. Device discovered via mDNS:");
    println!("   Name: {}", peer_info.name);
    println!("   Address: {}", peer_info.addresses[0]);
    println!(
        "   Has public key: {}",
        peer_info.metadata.ssh_public_key.is_some()
    );

    // 4. Demonstrate trust prompt (in real usage, this would be interactive)
    println!("\n4. Trust prompt simulation:");
    println!("   In a real scenario, you would see:");
    println!("   ");
    println!("   === New Device Discovered ===");
    println!("   Device Name: {}", peer_info.name);
    println!("   Device ID: {}", peer_info.id);
    println!("   Address: {}", peer_info.addresses[0]);
    println!("   SSH Fingerprint: {}", public_key.fingerprint());
    println!("   ");
    println!("   Do you want to trust this device?");
    println!("   [y] Yes, trust this device");
    println!("   [n] No, reject this device");
    println!("   [i] Ignore for now (ask again later)");
    println!("   ");
    println!("   Your choice [y/n/i]: y");

    // 5. Show the result
    println!("\n5. After trusting:");
    println!("   ✓ Device added to trusted devices list");
    println!("   ✓ Public key added to authorized_keys");
    println!("   ✓ Secure connection established");
    println!("   ✓ Clipboard syncing enabled!");

    println!("\n=== Setup Complete! ===");
    println!("The devices can now sync clipboards automatically.");
    println!("No manual SSH key exchange was required!");

    Ok(())
}
