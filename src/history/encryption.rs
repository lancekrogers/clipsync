//! AES-256-GCM encryption for clipboard history

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Result};
use argon2::{Argon2, PasswordHasher};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
use zeroize::{Zeroize, Zeroizing};
use zstd::stream::{decode_all, encode_all};

const COMPRESSION_THRESHOLD: usize = 100 * 1024; // 100KB
const COMPRESSION_LEVEL: i32 = 3;
const KEY_FILE_NAME: &str = "history.key";

/// AES-256-GCM encryptor with secure key management
pub struct Encryptor {
    cipher: Aes256Gcm,
    key: Zeroizing<[u8; 32]>,
}

/// Encrypted data container with metadata
#[derive(Debug)]
pub struct EncryptedData {
    /// The encrypted ciphertext
    pub ciphertext: Vec<u8>,
    /// The initialization vector used for encryption
    pub nonce: Vec<u8>,
    /// Whether the data was compressed before encryption
    pub compressed: bool,
}

impl Encryptor {
    /// Create a new encryptor with automatic key management
    pub async fn new() -> Result<Self> {
        let key = Self::load_or_create_key().await?;
        let cipher = Aes256Gcm::new_from_slice(&key)?;

        Ok(Self {
            cipher,
            key: Zeroizing::new(key),
        })
    }

    /// Get the encryption key (for database initialization)
    pub fn get_key(&self) -> &[u8; 32] {
        &self.key
    }

    /// Encrypt data with optional compression
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<EncryptedData> {
        let mut data = plaintext.to_vec();
        let compressed = data.len() > COMPRESSION_THRESHOLD;

        if compressed {
            data = encode_all(&data[..], COMPRESSION_LEVEL)?;
        }

        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, data.as_ref())
            .map_err(|e| anyhow!("Encryption failed: {}", e))?;

        data.zeroize();

        Ok(EncryptedData {
            ciphertext,
            nonce: nonce.to_vec(),
            compressed,
        })
    }

    /// Decrypt data and decompress if needed
    pub fn decrypt(&self, encrypted: &EncryptedData) -> Result<Vec<u8>> {
        let nonce = Nonce::from_slice(&encrypted.nonce);
        let mut plaintext = self
            .cipher
            .decrypt(nonce, encrypted.ciphertext.as_ref())
            .map_err(|e| anyhow!("Decryption failed: {}", e))?;

        if encrypted.compressed {
            let decompressed = decode_all(&plaintext[..])?;
            plaintext.zeroize();
            plaintext = decompressed;
        }

        Ok(plaintext)
    }

    /// Compute SHA-256 checksum of data
    pub fn compute_checksum(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    async fn load_or_create_key() -> Result<[u8; 32]> {
        // Use file-based key storage for cross-platform compatibility
        let key_path = Self::get_key_file_path()?;

        if key_path.exists() {
            // Try to load existing key
            match Self::load_from_file(&key_path).await {
                Ok(key) => return Ok(key),
                Err(e) => {
                    tracing::warn!("Failed to load existing key: {}", e);
                    // Continue to check for migration or generate new key
                }
            }
        }

        // Generate new key
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);

        // Save to file with proper permissions
        Self::save_to_file(&key_path, &key).await?;
        Ok(key)
    }

    async fn load_from_file(path: &Path) -> Result<[u8; 32]> {
        // Check file permissions on Unix systems
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(path)?;
            let mode = metadata.permissions().mode();
            if mode & 0o077 != 0 {
                return Err(anyhow!("Key file has insecure permissions: {:o}", mode));
            }
        }

        let data = fs::read(path)?;
        if data.len() != 32 {
            return Err(anyhow!(
                "Invalid key file: expected 32 bytes, got {}",
                data.len()
            ));
        }

        let mut key = [0u8; 32];
        key.copy_from_slice(&data);
        Ok(key)
    }

    async fn save_to_file(path: &Path, key: &[u8; 32]) -> Result<()> {
        // Create directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;

            // Set directory permissions on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(parent)?.permissions();
                perms.set_mode(0o700); // rwx for owner only
                fs::set_permissions(parent, perms)?;
            }
        }

        // Write key file
        fs::write(path, key)?;

        // Set restrictive permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path)?.permissions();
            perms.set_mode(0o600); // Read/write for owner only
            fs::set_permissions(path, perms)?;
        }

        // On Windows, file permissions are handled by default ACLs
        #[cfg(windows)]
        {
            // Windows file permissions are more complex and typically handled
            // by the default ACLs. The file will be created with permissions
            // inherited from the parent directory.
        }

        tracing::info!("Encryption key saved to {:?}", path);
        Ok(())
    }

    fn get_key_file_path() -> Result<PathBuf> {
        // Use platform-specific config directory
        let config_dir = if cfg!(target_os = "linux") {
            // On Linux, prefer XDG_CONFIG_HOME or ~/.config
            std::env::var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|_| {
                    dirs::home_dir()
                        .map(|home| home.join(".config"))
                        .ok_or_else(|| anyhow!("Could not determine home directory"))
                })?
        } else {
            // On macOS and other platforms, use the standard config directory
            dirs::config_dir().ok_or_else(|| anyhow!("Could not determine config directory"))?
        };

        Ok(config_dir.join("clipsync").join(KEY_FILE_NAME))
    }

    /// Derive encryption key from password using Argon2id
    pub async fn derive_from_password(password: &str) -> Result<[u8; 32]> {
        use argon2::password_hash::SaltString;
        use argon2::{Algorithm, Params, Version};

        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            Params::new(15000, 2, 1, Some(32))
                .map_err(|e| anyhow!("Invalid argon2 params: {}", e))?,
        );

        let hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| anyhow!("Key derivation failed: {}", e))?;

        let hash_bytes = hash.hash.ok_or_else(|| anyhow!("No hash output"))?;
        let bytes = hash_bytes.as_bytes();

        if bytes.len() != 32 {
            return Err(anyhow!("Invalid derived key length"));
        }

        let mut key = [0u8; 32];
        key.copy_from_slice(bytes);
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_encrypt_decrypt_roundtrip() {
        let encryptor = Encryptor::new().await.unwrap();
        let plaintext = b"Hello, world!";

        let encrypted = encryptor.encrypt(plaintext).unwrap();
        let decrypted = encryptor.decrypt(&encrypted).unwrap();

        assert_eq!(plaintext, &decrypted[..]);
    }

    #[tokio::test]
    async fn test_large_payload_compression() {
        let encryptor = Encryptor::new().await.unwrap();
        let large_data = vec![b'A'; 200 * 1024]; // 200KB of 'A's

        let encrypted = encryptor.encrypt(&large_data).unwrap();
        assert!(encrypted.compressed);

        let decrypted = encryptor.decrypt(&encrypted).unwrap();
        assert_eq!(large_data, decrypted);
    }

    #[test]
    fn test_checksum() {
        let data = b"test data";
        let checksum1 = Encryptor::compute_checksum(data);
        let checksum2 = Encryptor::compute_checksum(data);

        assert_eq!(checksum1, checksum2);

        let different_data = b"different data";
        let checksum3 = Encryptor::compute_checksum(different_data);
        assert_ne!(checksum1, checksum3);
    }

    #[tokio::test]
    async fn test_key_derivation() {
        let password = "test_password";
        let key1 = Encryptor::derive_from_password(password).await.unwrap();
        let key2 = Encryptor::derive_from_password(password).await.unwrap();

        // Different salts should produce different keys
        assert_ne!(key1, key2);
    }
}
