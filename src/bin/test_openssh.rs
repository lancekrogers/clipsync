use clipsync::auth::openssh::{parse_openssh_private_key, KeyTypeData};
use base64::Engine as _;
use std::fs;

fn main() {
    let key_path = "/home/lance/.ssh/id_ed25519";
    println!("Testing OpenSSH key parsing for: {}", key_path);
    
    // Read key file
    let key_data = fs::read_to_string(key_path).expect("Failed to read key file");
    
    // Extract base64 content
    let start = key_data.find("-----BEGIN OPENSSH PRIVATE KEY-----").expect("Missing start marker");
    let end = key_data.find("-----END OPENSSH PRIVATE KEY-----").expect("Missing end marker");
    let base64_content = &key_data[start + 35..end];
    
    // Decode base64
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(base64_content.replace(['\n', '\r'], ""))
        .expect("Failed to decode base64");
    
    println!("Decoded {} bytes", decoded.len());
    
    // Parse OpenSSH format
    match parse_openssh_private_key(&decoded) {
        Ok(openssh_key) => {
            println!("Successfully parsed OpenSSH key!");
            println!("Cipher: {}", openssh_key.cipher_name);
            println!("KDF: {}", openssh_key.kdf_name);
            println!("Encrypted: {}", openssh_key.is_encrypted);
            println!("Public key size: {} bytes", openssh_key.public_key.len());
            println!("Private section size: {} bytes", openssh_key.private_section.len());
            
            // Try to parse private section
            match clipsync::auth::openssh::parser::parse_private_section(&openssh_key.private_section) {
                Ok(private_data) => {
                    println!("\nPrivate key parsed successfully!");
                    println!("Key type: {}", private_data.key_type);
                    println!("Comment: {}", private_data.comment);
                    
                    match &private_data.private_data {
                        KeyTypeData::Ed25519 { public, private } => {
                            println!("Ed25519 public key: {} bytes", public.len());
                            println!("Ed25519 private key: {} bytes", private.len());
                            
                            // Try PKCS8 conversion
                            match clipsync::auth::openssh::parser::ed25519_to_pkcs8(&private_data.private_data) {
                                Ok(pkcs8) => {
                                    println!("\nPKCS8 conversion successful!");
                                    println!("PKCS8 size: {} bytes", pkcs8.len());
                                    println!("PKCS8 hex: {}", hex::encode(&pkcs8));
                                    
                                    // Try to load with ring
                                    match ring::signature::Ed25519KeyPair::from_pkcs8(&pkcs8) {
                                        Ok(_) => println!("Ring successfully loaded the PKCS8 key!"),
                                        Err(e) => println!("Ring failed to load PKCS8: {:?}", e),
                                    }
                                }
                                Err(e) => println!("PKCS8 conversion failed: {}", e),
                            }
                        }
                        _ => println!("Not an Ed25519 key"),
                    }
                }
                Err(e) => println!("Failed to parse private section: {}", e),
            }
        }
        Err(e) => println!("Failed to parse OpenSSH key: {}", e),
    }
}