//! OpenSSH private key format parser

use std::io::{Cursor, Read};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OpenSshError {
    #[error("Invalid magic header")]
    InvalidMagic,
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Invalid UTF-8: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    
    #[error("Multiple keys not supported")]
    MultipleKeysNotSupported,
    
    #[error("Invalid check values")]
    InvalidCheckValues,
    
    #[error("Unsupported key type: {0}")]
    UnsupportedKeyType(String),
    
    #[error("Invalid key size")]
    InvalidKeySize,
    
    #[error("Invalid padding")]
    InvalidPadding,
}

/// Parsed OpenSSH key structure
pub struct OpenSshKey {
    pub cipher_name: String,
    pub kdf_name: String,
    pub kdf_options: Vec<u8>,
    pub public_key: Vec<u8>,
    pub private_section: Vec<u8>,
    pub is_encrypted: bool,
}

/// Private key data after parsing
pub struct PrivateKeyData {
    pub key_type: String,
    pub private_data: KeyTypeData,
    pub comment: String,
}

/// Key type specific data
pub enum KeyTypeData {
    Ed25519 {
        public: [u8; 32],
        private: [u8; 64],  // 32-byte private + 32-byte public
    },
    Rsa {
        n: Vec<u8>,  // modulus
        e: Vec<u8>,  // public exponent
        d: Vec<u8>,  // private exponent
        iqmp: Vec<u8>,  // q^-1 mod p
        p: Vec<u8>,  // prime1
        q: Vec<u8>,  // prime2
    },
}

/// Parse OpenSSH private key from binary data
pub fn parse_openssh_private_key(data: &[u8]) -> Result<OpenSshKey, OpenSshError> {
    let mut cursor = Cursor::new(data);
    
    // Read and verify magic
    let mut magic = [0u8; 14];
    cursor.read_exact(&mut magic)?;
    if &magic != b"openssh-key-v1" {
        return Err(OpenSshError::InvalidMagic);
    }
    
    // Skip null terminator
    let mut null = [0u8; 1];
    cursor.read_exact(&mut null)?;
    
    // Read cipher name
    let cipher_name = read_string(&mut cursor)?;
    let kdf_name = read_string(&mut cursor)?;
    let kdf_options = read_bytes(&mut cursor)?;
    
    // Read number of keys
    let num_keys = read_u32(&mut cursor)?;
    if num_keys != 1 {
        return Err(OpenSshError::MultipleKeysNotSupported);
    }
    
    // Read public key
    let public_key = read_bytes(&mut cursor)?;
    
    // Read private section
    let private_section = read_bytes(&mut cursor)?;
    
    Ok(OpenSshKey {
        is_encrypted: cipher_name != "none",
        cipher_name,
        kdf_name,
        kdf_options,
        public_key,
        private_section,
    })
}

/// Parse the private section of an OpenSSH key
pub fn parse_private_section(data: &[u8]) -> Result<PrivateKeyData, OpenSshError> {
    let mut cursor = Cursor::new(data);
    
    // Read and verify check values
    let check1 = read_u32(&mut cursor)?;
    let check2 = read_u32(&mut cursor)?;
    if check1 != check2 {
        return Err(OpenSshError::InvalidCheckValues);
    }
    
    // Read key type
    let key_type = read_string(&mut cursor)?;
    
    // Parse key-specific data
    let private_data = match key_type.as_str() {
        "ssh-ed25519" => parse_ed25519_private(&mut cursor)?,
        "ssh-rsa" => parse_rsa_private(&mut cursor)?,
        _ => return Err(OpenSshError::UnsupportedKeyType(key_type)),
    };
    
    // Read comment
    let comment = read_string(&mut cursor)?;
    
    // Verify padding
    verify_padding(&mut cursor)?;
    
    Ok(PrivateKeyData {
        key_type,
        private_data,
        comment,
    })
}

/// Parse Ed25519 private key data
fn parse_ed25519_private(cursor: &mut Cursor<&[u8]>) -> Result<KeyTypeData, OpenSshError> {
    // Read public key (32 bytes)
    let public_bytes = read_bytes(cursor)?;
    if public_bytes.len() != 32 {
        return Err(OpenSshError::InvalidKeySize);
    }
    
    // Read private key (64 bytes: 32 private + 32 public)
    let private_bytes = read_bytes(cursor)?;
    if private_bytes.len() != 64 {
        return Err(OpenSshError::InvalidKeySize);
    }
    
    let mut public = [0u8; 32];
    let mut private = [0u8; 64];
    public.copy_from_slice(&public_bytes);
    private.copy_from_slice(&private_bytes);
    
    Ok(KeyTypeData::Ed25519 { public, private })
}

/// Parse RSA private key data
fn parse_rsa_private(cursor: &mut Cursor<&[u8]>) -> Result<KeyTypeData, OpenSshError> {
    let n = read_bytes(cursor)?;  // modulus
    let e = read_bytes(cursor)?;  // public exponent
    let d = read_bytes(cursor)?;  // private exponent
    let iqmp = read_bytes(cursor)?;  // q^-1 mod p
    let p = read_bytes(cursor)?;  // prime1
    let q = read_bytes(cursor)?;  // prime2
    
    Ok(KeyTypeData::Rsa { n, e, d, iqmp, p, q })
}

/// Read a string from the cursor (4-byte length prefix + data)
fn read_string(cursor: &mut Cursor<&[u8]>) -> Result<String, OpenSshError> {
    let bytes = read_bytes(cursor)?;
    Ok(String::from_utf8(bytes)?)
}

/// Read bytes from the cursor (4-byte length prefix + data)
fn read_bytes(cursor: &mut Cursor<&[u8]>) -> Result<Vec<u8>, OpenSshError> {
    let len = read_u32(cursor)? as usize;
    let mut bytes = vec![0u8; len];
    cursor.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// Read a big-endian u32 from the cursor
fn read_u32(cursor: &mut Cursor<&[u8]>) -> Result<u32, OpenSshError> {
    let mut bytes = [0u8; 4];
    cursor.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

/// Verify the padding at the end of the private section
fn verify_padding(cursor: &mut Cursor<&[u8]>) -> Result<(), OpenSshError> {
    let position = cursor.position() as usize;
    let data = cursor.get_ref();
    let remaining = &data[position..];
    
    if remaining.is_empty() {
        return Ok(());
    }
    
    // Padding should be 1, 2, 3, ..., n
    for (i, &byte) in remaining.iter().enumerate() {
        if byte != ((i + 1) as u8) {
            return Err(OpenSshError::InvalidPadding);
        }
    }
    
    Ok(())
}

/// Convert Ed25519 OpenSSH key to PKCS8 format
pub fn ed25519_to_pkcs8(key_data: &KeyTypeData) -> Result<Vec<u8>, OpenSshError> {
    match key_data {
        KeyTypeData::Ed25519 { private, .. } => {
            // Extract the 32-byte seed (first half of private key)
            let seed = &private[..32];
            
            // PKCS8 v1 structure for Ed25519 (OneAsymmetricKey) with public key
            // This is what ring expects - it includes the public key in attributes
            let public_key_bytes = &private[32..64]; // Second half is public key
            
            let mut pkcs8 = vec![
                0x30, 0x51,  // SEQUENCE (81 bytes total)
                0x02, 0x01, 0x01,  // version INTEGER 1 (v2)
                0x30, 0x05,  // AlgorithmIdentifier SEQUENCE
                0x06, 0x03, 0x2b, 0x65, 0x70,  // OID 1.3.101.112 (Ed25519)
                0x04, 0x22,  // PrivateKey OCTET STRING (34 bytes)
                0x04, 0x20,  // Inner OCTET STRING (32 bytes) containing the key
            ];
            pkcs8.extend_from_slice(seed);
            
            // Add attributes [1] with public key - Ring's format
            pkcs8.extend_from_slice(&[
                0x81, 0x21,  // [1] IMPLICIT (33 bytes)
                0x00,        // First byte is 0x00
            ]);
            pkcs8.extend_from_slice(public_key_bytes);
            
            Ok(pkcs8)
        }
        _ => Err(OpenSshError::UnsupportedKeyType("Not Ed25519".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_read_u32() {
        let data = vec![0x00, 0x00, 0x00, 0x05];
        let mut cursor = Cursor::new(data.as_slice());
        assert_eq!(read_u32(&mut cursor).unwrap(), 5);
    }
    
    #[test]
    fn test_read_string() {
        let data = vec![
            0x00, 0x00, 0x00, 0x05,  // length = 5
            b'h', b'e', b'l', b'l', b'o'
        ];
        let mut cursor = Cursor::new(data.as_slice());
        assert_eq!(read_string(&mut cursor).unwrap(), "hello");
    }
    
    #[test]
    fn test_verify_padding() {
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let cursor = Cursor::new(data.as_slice());
        assert!(verify_padding(&mut cursor.clone()).is_ok());
        
        let bad_data = vec![1, 2, 3, 4, 5, 6, 7, 0];
        let bad_cursor = Cursor::new(bad_data.as_slice());
        assert!(verify_padding(&mut bad_cursor.clone()).is_err());
    }
}