//! SSH key management utilities

use crate::auth::AuthError;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as RingKeyPair};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// SSH key type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyType {
    /// Ed25519 (recommended)
    Ed25519,
    /// RSA (for compatibility)
    #[allow(dead_code)]
    Rsa,
}

impl KeyType {
    /// Get the SSH key type string
    pub fn ssh_name(&self) -> &'static str {
        match self {
            KeyType::Ed25519 => "ssh-ed25519",
            KeyType::Rsa => "ssh-rsa",
        }
    }
}

/// SSH public key
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey {
    /// Key type
    pub key_type: KeyType,
    /// Raw public key bytes
    pub key_data: Vec<u8>,
}

impl PublicKey {
    /// Create a new public key
    pub fn new(key_type: KeyType, key_data: Vec<u8>) -> Self {
        Self { key_type, key_data }
    }

    /// Get the fingerprint of the public key (SHA256 hash)
    pub fn fingerprint(&self) -> String {
        use ring::digest;
        let hash = digest::digest(&digest::SHA256, &self.key_data);
        let encoded = BASE64.encode(hash.as_ref());
        format!("SHA256:{}", encoded.trim_end_matches('='))
    }

    /// Export to OpenSSH format
    pub fn to_openssh(&self) -> String {
        let encoded = BASE64.encode(&self.key_data);
        format!("{} {}", self.key_type.ssh_name(), encoded)
    }

    /// Parse from OpenSSH format
    pub fn from_openssh(openssh_key: &str) -> Result<Self, AuthError> {
        let parts: Vec<&str> = openssh_key.trim().split_whitespace().collect();
        if parts.len() < 2 {
            return Err(AuthError::InvalidKeyFormat(
                "Invalid OpenSSH key format".to_string(),
            ));
        }

        let key_type = match parts[0] {
            "ssh-ed25519" => KeyType::Ed25519,
            "ssh-rsa" => KeyType::Rsa,
            _ => {
                return Err(AuthError::InvalidKeyFormat(format!(
                    "Unsupported key type: {}",
                    parts[0]
                )))
            }
        };

        let key_data = BASE64
            .decode(parts[1])
            .map_err(|e| AuthError::InvalidKeyFormat(format!("Invalid base64: {}", e)))?;

        Ok(Self::new(key_type, key_data))
    }

    /// Verify a signature
    pub fn verify(&self, message: &[u8], signature: &[u8]) -> Result<bool, AuthError> {
        match self.key_type {
            KeyType::Ed25519 => {
                use ring::signature::{UnparsedPublicKey, ED25519};
                let public_key = UnparsedPublicKey::new(&ED25519, &self.key_data);
                match public_key.verify(message, signature) {
                    Ok(()) => Ok(true),
                    Err(_) => Ok(false),
                }
            }
            KeyType::Rsa => Err(AuthError::KeyError(
                "RSA verification not implemented".to_string(),
            )),
        }
    }

    /// Export to OpenSSH wire format (binary format)
    pub fn to_openssh_format(&self) -> Vec<u8> {
        // Simplified - returns the key data directly
        // In a real implementation, this would encode according to RFC 4251
        self.key_data.clone()
    }

    /// Parse from OpenSSH wire format (binary format)
    pub fn from_openssh_format(data: &[u8]) -> Result<Self, AuthError> {
        // Simplified - assumes Ed25519 key data
        // In a real implementation, this would parse according to RFC 4251
        Ok(Self::new(KeyType::Ed25519, data.to_vec()))
    }
}

/// SSH key pair
pub struct KeyPair {
    /// Key type
    pub key_type: KeyType,
    /// Private key material
    private_key: Vec<u8>,
    /// Public key
    public_key: PublicKey,
}

impl KeyPair {
    /// Generate a new key pair
    pub fn generate(key_type: KeyType) -> Result<Self, AuthError> {
        match key_type {
            KeyType::Ed25519 => {
                let rng = SystemRandom::new();
                let pkcs8_bytes = Ed25519KeyPair::generate_pkcs8(&rng)
                    .map_err(|e| AuthError::CryptoError(e.to_string()))?;

                let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8_bytes.as_ref())
                    .map_err(|e| AuthError::CryptoError(e.to_string()))?;

                let public_key_bytes = key_pair.public_key().as_ref().to_vec();

                Ok(Self {
                    key_type,
                    private_key: pkcs8_bytes.as_ref().to_vec(),
                    public_key: PublicKey::new(key_type, public_key_bytes),
                })
            }
            KeyType::Rsa => Err(AuthError::KeyError(
                "RSA key generation not implemented".to_string(),
            )),
        }
    }

    /// Load from private key file
    pub async fn load_from_file(path: &Path) -> Result<Self, AuthError> {
        let key_data = tokio::fs::read(path).await?;
        Self::from_private_key_bytes(&key_data)
    }

    /// Save to private key file
    pub async fn save_to_file(&self, path: &Path) -> Result<(), AuthError> {
        // Ensure directory exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Write private key with restricted permissions
        use tokio::fs::OpenOptions;

        let mut file = {
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                #[allow(unused_imports)]
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600); // Owner read/write only
            }
            options.open(path).await?
        };

        use tokio::io::AsyncWriteExt;

        // Write PKCS8 PEM format
        let pem_content = self.to_pkcs8_pem()?;
        file.write_all(pem_content.as_bytes()).await?;

        // Also write public key
        let pub_path = path.with_extension("pub");
        let pub_content = format!("{} clipsync@localhost\n", self.public_key.to_openssh());
        tokio::fs::write(&pub_path, pub_content).await?;

        Ok(())
    }

    /// Parse from private key bytes
    pub fn from_private_key_bytes(key_data: &[u8]) -> Result<Self, AuthError> {
        // Try to parse as PKCS8 Ed25519
        if let Ok(key_pair) = Ed25519KeyPair::from_pkcs8(key_data) {
            let public_key_bytes = key_pair.public_key().as_ref().to_vec();
            return Ok(Self {
                key_type: KeyType::Ed25519,
                private_key: key_data.to_vec(),
                public_key: PublicKey::new(KeyType::Ed25519, public_key_bytes),
            });
        }

        // Check if this might be a text-based key format (PEM)
        // Only try UTF-8 conversion if the data looks like it might be text
        // (starts with ASCII characters typical of PEM headers)
        if key_data.len() > 5 && key_data[0..5].iter().all(|&b| b.is_ascii()) {
            if let Ok(key_str) = std::str::from_utf8(key_data) {
                if key_str.contains("BEGIN OPENSSH PRIVATE KEY") {
                    // Parse OpenSSH format (simplified for Ed25519)
                    return Self::from_openssh_private_key(key_str);
                }
                
                // Check for PKCS8 PEM format
                if key_str.contains("BEGIN PRIVATE KEY") {
                    return Self::from_pkcs8_pem(key_str);
                }
                
                // Check for other PEM formats
                if key_str.contains("BEGIN RSA PRIVATE KEY") {
                    // For now, we don't support PKCS#1 format
                    return Err(AuthError::InvalidKeyFormat(
                        "RSA PKCS#1 private keys are not yet supported".to_string(),
                    ));
                }
            }
        }

        Err(AuthError::InvalidKeyFormat(
            "Unsupported private key format".to_string(),
        ))
    }

    /// Convert to PKCS8 PEM format
    fn to_pkcs8_pem(&self) -> Result<String, AuthError> {
        match self.key_type {
            KeyType::Ed25519 => {
                // Encode PKCS8 data in standard PEM format
                let encoded = BASE64.encode(&self.private_key);
                let pem =
                    format!(
                    "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
                    encoded.chars().collect::<Vec<_>>().chunks(64)
                        .map(|chunk| chunk.iter().collect::<String>())
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                Ok(pem)
            }
            KeyType::Rsa => Err(AuthError::KeyError(
                "RSA export not implemented".to_string(),
            )),
        }
    }

    /// Parse PKCS8 PEM format
    fn from_pkcs8_pem(pem: &str) -> Result<Self, AuthError> {
        // Extract base64 content between markers
        let start_marker = "-----BEGIN PRIVATE KEY-----";
        let end_marker = "-----END PRIVATE KEY-----";

        let start = pem
            .find(start_marker)
            .ok_or_else(|| AuthError::InvalidKeyFormat("Missing start marker".to_string()))?;
        let end = pem
            .find(end_marker)
            .ok_or_else(|| AuthError::InvalidKeyFormat("Missing end marker".to_string()))?;

        let base64_content = &pem[start + start_marker.len()..end];
        let decoded = BASE64
            .decode(base64_content.replace(['\n', '\r'], ""))
            .map_err(|e| AuthError::InvalidKeyFormat(format!("Invalid base64: {}", e)))?;

        // Try to parse as PKCS8
        if let Ok(key_pair) = Ed25519KeyPair::from_pkcs8(&decoded) {
            let public_key_bytes = key_pair.public_key().as_ref().to_vec();
            return Ok(Self {
                key_type: KeyType::Ed25519,
                private_key: decoded,
                public_key: PublicKey::new(KeyType::Ed25519, public_key_bytes),
            });
        }

        Err(AuthError::InvalidKeyFormat(
            "Invalid PKCS8 private key".to_string(),
        ))
    }

    /// Parse OpenSSH private key format
    fn from_openssh_private_key(pem: &str) -> Result<Self, AuthError> {
        // Extract base64 content between markers
        let start_marker = "-----BEGIN OPENSSH PRIVATE KEY-----";
        let end_marker = "-----END OPENSSH PRIVATE KEY-----";

        let start = pem
            .find(start_marker)
            .ok_or_else(|| AuthError::InvalidKeyFormat("Missing start marker".to_string()))?;
        let end = pem
            .find(end_marker)
            .ok_or_else(|| AuthError::InvalidKeyFormat("Missing end marker".to_string()))?;

        let base64_content = &pem[start + start_marker.len()..end];
        let decoded = BASE64
            .decode(base64_content.replace(['\n', '\r'], ""))
            .map_err(|e| AuthError::InvalidKeyFormat(format!("Invalid base64: {}", e)))?;

        // OpenSSH format has a specific structure - we need to parse it properly
        // For now, we only support Ed25519 keys stored in OpenSSH format
        // The format contains a header, followed by the private key material
        
        // Check for OpenSSH magic bytes "openssh-key-v1\0"
        const OPENSSH_MAGIC: &[u8] = b"openssh-key-v1\0";
        if decoded.len() < OPENSSH_MAGIC.len() || &decoded[..OPENSSH_MAGIC.len()] != OPENSSH_MAGIC {
            return Err(AuthError::InvalidKeyFormat(
                "Invalid OpenSSH private key format: missing magic bytes".to_string(),
            ));
        }
        
        // For now, we don't fully parse the OpenSSH format structure
        // Instead, we return an error indicating it's not yet supported
        // A full implementation would need to parse the OpenSSH format structure
        // which includes cipher info, kdf info, number of keys, public key, private key, etc.
        Err(AuthError::InvalidKeyFormat(
            "OpenSSH format private keys are not yet fully supported. Please use PKCS8 format for Ed25519 keys or convert your key.".to_string(),
        ))
    }

    /// Get the public key
    pub fn public_key(&self) -> PublicKey {
        self.public_key.clone()
    }

    /// Sign a message
    pub fn sign(&self, message: &[u8]) -> Result<Vec<u8>, AuthError> {
        match self.key_type {
            KeyType::Ed25519 => {
                let key_pair = Ed25519KeyPair::from_pkcs8(&self.private_key)
                    .map_err(|e| AuthError::CryptoError(e.to_string()))?;
                Ok(key_pair.sign(message).as_ref().to_vec())
            }
            KeyType::Rsa => Err(AuthError::KeyError(
                "RSA signing not implemented".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_key_generation() {
        let key_pair = KeyPair::generate(KeyType::Ed25519).unwrap();
        assert_eq!(key_pair.key_type, KeyType::Ed25519);
        assert!(!key_pair.private_key.is_empty());
        assert!(!key_pair.public_key.key_data.is_empty());
    }

    #[test]
    fn test_public_key_fingerprint() {
        let key_pair = KeyPair::generate(KeyType::Ed25519).unwrap();
        let fingerprint = key_pair.public_key().fingerprint();
        assert!(fingerprint.starts_with("SHA256:"));
        assert!(fingerprint.len() > 10);
    }

    #[test]
    fn test_openssh_format() {
        let key_pair = KeyPair::generate(KeyType::Ed25519).unwrap();
        let openssh = key_pair.public_key().to_openssh();
        assert!(openssh.starts_with("ssh-ed25519 "));

        // Parse it back
        let parsed = PublicKey::from_openssh(&openssh).unwrap();
        assert_eq!(parsed.key_type, KeyType::Ed25519);
        assert_eq!(parsed.key_data, key_pair.public_key.key_data);
    }

    #[test]
    fn test_sign_verify() {
        let key_pair = KeyPair::generate(KeyType::Ed25519).unwrap();
        let message = b"Hello, ClipSync!";

        let signature = key_pair.sign(message).unwrap();
        assert!(!signature.is_empty());

        let verified = key_pair.public_key().verify(message, &signature).unwrap();
        assert!(verified);

        // Wrong message should fail
        let wrong_message = b"Wrong message";
        let verified = key_pair
            .public_key()
            .verify(wrong_message, &signature)
            .unwrap();
        assert!(!verified);
    }

    #[tokio::test]
    async fn test_save_load_key() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("test_key");

        // Generate and save
        let key_pair = KeyPair::generate(KeyType::Ed25519).unwrap();
        let original_fingerprint = key_pair.public_key().fingerprint();
        key_pair.save_to_file(&key_path).await.unwrap();

        // Load back
        let loaded_key = KeyPair::load_from_file(&key_path).await.unwrap();
        let loaded_fingerprint = loaded_key.public_key().fingerprint();

        assert_eq!(original_fingerprint, loaded_fingerprint);

        // Check public key file was created
        let pub_path = key_path.with_extension("pub");
        assert!(pub_path.exists());
    }
}
