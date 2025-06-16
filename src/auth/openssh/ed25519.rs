//! Ed25519 specific OpenSSH key handling

use super::{KeyTypeData};
use super::parser::OpenSshError;
use ring::rand::SystemRandom;
use ring::signature::Ed25519KeyPair;

/// Convert OpenSSH Ed25519 key to PKCS8 format
pub fn ed25519_openssh_to_pkcs8(key_data: &KeyTypeData) -> Result<Vec<u8>, OpenSshError> {
    match key_data {
        KeyTypeData::Ed25519 { private, .. } => {
            // In OpenSSH format, the private key is 64 bytes:
            // - First 32 bytes: the actual private key (seed)
            // - Last 32 bytes: the public key
            let seed = &private[..32];
            
            // Generate a new keypair from the seed
            // This ensures we get the exact PKCS8 format that ring expects
            let rng = SystemRandom::new();
            
            // Unfortunately, ring doesn't provide a way to create a keypair from a seed directly
            // We need to manually construct the PKCS8 document
            
            // Let's try the minimal PKCS8 v0 format that some versions of ring accept
            let pkcs8_v0 = create_pkcs8_v0(seed);
            
            // Test if this works
            if Ed25519KeyPair::from_pkcs8(&pkcs8_v0).is_ok() {
                return Ok(pkcs8_v0);
            }
            
            // If not, try v1 format with public key
            let public_key = &private[32..64];
            let pkcs8_v1 = create_pkcs8_v1(seed, public_key);
            
            // Test this format
            if Ed25519KeyPair::from_pkcs8(&pkcs8_v1).is_ok() {
                return Ok(pkcs8_v1);
            }
            
            // If neither works, we have a problem
            Err(OpenSshError::InvalidKeySize)
        }
        _ => Err(OpenSshError::UnsupportedKeyType("Not Ed25519".to_string())),
    }
}

/// Create PKCS8 v0 format (minimal)
fn create_pkcs8_v0(seed: &[u8]) -> Vec<u8> {
    let mut pkcs8 = vec![
        0x30, 0x2e,  // SEQUENCE (46 bytes)
        0x02, 0x01, 0x00,  // version INTEGER 0
        0x30, 0x05,  // AlgorithmIdentifier SEQUENCE
        0x06, 0x03, 0x2b, 0x65, 0x70,  // OID 1.3.101.112 (Ed25519)
        0x04, 0x22,  // PrivateKey OCTET STRING (34 bytes)
        0x04, 0x20,  // Inner OCTET STRING (32 bytes)
    ];
    pkcs8.extend_from_slice(seed);
    pkcs8
}

/// Create PKCS8 v1 format (with public key in attributes)
fn create_pkcs8_v1(seed: &[u8], public_key: &[u8]) -> Vec<u8> {
    // This matches the structure that ring generates
    let mut pkcs8 = Vec::new();
    
    // Calculate total length: 2 + 3 + 2 + 5 + 2 + 34 + 2 + 1 + 32 = 83
    pkcs8.extend_from_slice(&[0x30, 0x51]); // SEQUENCE (81 bytes)
    
    // Version
    pkcs8.extend_from_slice(&[0x02, 0x01, 0x01]); // INTEGER 1
    
    // AlgorithmIdentifier
    pkcs8.extend_from_slice(&[
        0x30, 0x05,  // SEQUENCE (5 bytes)
        0x06, 0x03, 0x2b, 0x65, 0x70,  // OID
    ]);
    
    // PrivateKey
    pkcs8.extend_from_slice(&[0x04, 0x22]); // OCTET STRING (34 bytes)
    pkcs8.extend_from_slice(&[0x04, 0x20]); // Inner OCTET STRING (32 bytes)
    pkcs8.extend_from_slice(seed);
    
    // Attributes [1] IMPLICIT - Ring's format
    pkcs8.extend_from_slice(&[0x81, 0x21]); // [1] (33 bytes)
    pkcs8.push(0x00); // First byte is 0x00
    pkcs8.extend_from_slice(public_key);
    
    pkcs8
}