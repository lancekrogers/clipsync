use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};

fn main() {
    println!("Testing Ring Ed25519 PKCS8 generation and format");
    
    // Generate a new Ed25519 key pair
    let rng = SystemRandom::new();
    let pkcs8_bytes = Ed25519KeyPair::generate_pkcs8(&rng)
        .expect("Failed to generate PKCS8");
    
    println!("Generated PKCS8 size: {} bytes", pkcs8_bytes.as_ref().len());
    println!("Generated PKCS8 hex: {}", hex::encode(pkcs8_bytes.as_ref()));
    
    // Analyze the structure
    let bytes = pkcs8_bytes.as_ref();
    println!("\nPKCS8 Structure Analysis:");
    println!("  [0..2]: {:02x} {:02x} (SEQUENCE tag and length)", bytes[0], bytes[1]);
    println!("  [2..5]: {:02x} {:02x} {:02x} (version)", bytes[2], bytes[3], bytes[4]);
    println!("  [5..7]: {:02x} {:02x} (AlgorithmIdentifier SEQUENCE)", bytes[5], bytes[6]);
    println!("  [7..12]: {:02x} {:02x} {:02x} {:02x} {:02x} (OID)", bytes[7], bytes[8], bytes[9], bytes[10], bytes[11]);
    println!("  [12..14]: {:02x} {:02x} (PrivateKey OCTET STRING)", bytes[12], bytes[13]);
    println!("  [14..16]: {:02x} {:02x} (Inner OCTET STRING)", bytes[14], bytes[15]);
    
    // Show the private key seed
    let seed_start = 16;
    let seed_end = seed_start + 32;
    println!("  [16..48]: {} (32-byte seed)", hex::encode(&bytes[seed_start..seed_end]));
    
    // Check for attributes
    if bytes.len() > 48 {
        println!("  [48..50]: {:02x} {:02x} (Attributes tag)", bytes[48], bytes[49]);
        if bytes.len() > 50 {
            println!("  Remaining: {}", hex::encode(&bytes[50..]));
        }
    }
    
    // Verify it loads
    let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8_bytes.as_ref())
        .expect("Failed to load PKCS8");
    
    let public_key = key_pair.public_key();
    println!("\nLoaded successfully!");
    println!("Public key size: {} bytes", public_key.as_ref().len());
    println!("Public key: {}", hex::encode(public_key.as_ref()));
    
    // Now let's test our format
    println!("\n\nComparing with our PKCS8 format:");
    let our_pkcs8 = hex::decode("3051020101300506032b657004220420efe6fd153c34eb28be280196408e6a112f8fcf2117882a2c254dd9f1de8c60faa123032100dc850c2410d21f259be4e8b91130b02e0277d73ca23654e5f1df61c6e16b7339").unwrap();
    
    println!("Our PKCS8 size: {} bytes", our_pkcs8.len());
    println!("Our PKCS8 structure:");
    println!("  [0..2]: {:02x} {:02x} (SEQUENCE tag and length)", our_pkcs8[0], our_pkcs8[1]);
    println!("  [2..5]: {:02x} {:02x} {:02x} (version)", our_pkcs8[2], our_pkcs8[3], our_pkcs8[4]);
    
    // Compare byte by byte with Ring's format
    let min_len = std::cmp::min(bytes.len(), our_pkcs8.len());
    for i in 0..min_len {
        if bytes[i] != our_pkcs8[i] {
            println!("  First difference at byte {}: Ring={:02x}, Ours={:02x}", i, bytes[i], our_pkcs8[i]);
            break;
        }
    }
}