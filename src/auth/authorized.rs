//! SSH authorized_keys file management

use crate::auth::{AuthError, PublicKey};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

/// An authorized key entry
#[derive(Debug, Clone)]
pub struct AuthorizedKey {
    /// The public key
    pub public_key: PublicKey,
    /// Optional comment
    pub comment: Option<String>,
    /// SSH options (e.g., "no-port-forwarding", "command=...")
    pub options: Vec<String>,
}

impl AuthorizedKey {
    /// Convert to authorized_keys line format
    pub fn to_line(&self) -> String {
        let mut parts = Vec::new();

        // Add options if any
        if !self.options.is_empty() {
            parts.push(self.options.join(","));
        }

        // Add key
        parts.push(self.public_key.to_openssh());

        // Add comment if present
        if let Some(comment) = &self.comment {
            parts.push(comment.clone());
        }

        parts.join(" ")
    }

    /// Parse from authorized_keys line format
    pub fn from_line(line: &str) -> Result<Self, AuthError> {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') {
            return Err(AuthError::InvalidKeyFormat(
                "Empty or comment line".to_string(),
            ));
        }

        // Split into parts, handling quotes for options
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;
        let mut chars = line.chars().peekable();

        while let Some(ch) = chars.next() {
            match ch {
                '"' => in_quotes = !in_quotes,
                ' ' if !in_quotes => {
                    if !current.is_empty() {
                        parts.push(current.clone());
                        current.clear();
                    }
                }
                _ => current.push(ch),
            }
        }
        if !current.is_empty() {
            parts.push(current);
        }

        if parts.is_empty() {
            return Err(AuthError::InvalidKeyFormat("No parts found".to_string()));
        }

        // Find the key type and key data
        let mut key_type_index = None;
        for (i, part) in parts.iter().enumerate() {
            if part.starts_with("ssh-") || part.starts_with("ecdsa-") {
                key_type_index = Some(i);
                break;
            }
        }

        let key_type_index = key_type_index
            .ok_or_else(|| AuthError::InvalidKeyFormat("No SSH key type found".to_string()))?;

        // Parse options (everything before key type)
        let options: Vec<String> = if key_type_index > 0 {
            parts[0].split(',').map(|s| s.to_string()).collect()
        } else {
            Vec::new()
        };

        // Parse key
        let key_type = &parts[key_type_index];
        let key_data = parts
            .get(key_type_index + 1)
            .ok_or_else(|| AuthError::InvalidKeyFormat("No key data found".to_string()))?;

        let openssh_key = format!("{} {}", key_type, key_data);
        let public_key = PublicKey::from_openssh(&openssh_key)?;

        // Parse comment (everything after key data)
        let comment = if parts.len() > key_type_index + 2 {
            Some(parts[key_type_index + 2..].join(" "))
        } else {
            None
        };

        Ok(Self {
            public_key,
            comment,
            options,
        })
    }
}

/// Authorized keys manager
#[derive(Debug)]
pub struct AuthorizedKeys {
    /// List of authorized keys
    keys: Vec<AuthorizedKey>,
}

impl AuthorizedKeys {
    /// Create a new empty authorized keys list
    pub fn new() -> Self {
        Self { keys: Vec::new() }
    }

    /// Load from file
    pub async fn load_from_file(path: &Path) -> Result<Self, AuthError> {
        let file = tokio::fs::File::open(path).await?;
        let reader = tokio::io::BufReader::new(file);
        let mut lines = reader.lines();

        let mut keys = Vec::new();
        let mut line_num = 0;

        while let Some(line) = lines.next_line().await? {
            line_num += 1;

            // Skip empty lines and comments
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            match AuthorizedKey::from_line(&line) {
                Ok(key) => keys.push(key),
                Err(e) => {
                    eprintln!("Warning: Skipping invalid key at line {}: {}", line_num, e);
                }
            }
        }

        Ok(Self { keys })
    }

    /// Save to file
    pub async fn save_to_file(&self, path: &Path) -> Result<(), AuthError> {
        // Ensure directory exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Write file with restricted permissions
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

        // Write header
        file.write_all(b"# ClipSync authorized keys file\n").await?;
        file.write_all(
            b"# This file contains public keys authorized to connect to this ClipSync instance\n\n",
        )
        .await?;

        // Write keys
        for key in &self.keys {
            file.write_all(key.to_line().as_bytes()).await?;
            file.write_all(b"\n").await?;
        }

        Ok(())
    }

    /// Add a key
    pub fn add_key(&mut self, key: AuthorizedKey) {
        // Check if key already exists
        if !self.is_authorized(&key.public_key) {
            self.keys.push(key);
        }
    }

    /// Add a key from OpenSSH string format
    pub fn add_key_from_openssh(
        &mut self,
        openssh_key: &str,
        comment: Option<String>,
    ) -> Result<(), AuthError> {
        let public_key = PublicKey::from_openssh(openssh_key)?;
        self.add_key(AuthorizedKey {
            public_key,
            comment,
            options: vec![],
        });
        Ok(())
    }

    /// Remove a key by fingerprint
    pub fn remove_key_by_fingerprint(&mut self, fingerprint: &str) -> bool {
        let initial_len = self.keys.len();
        self.keys
            .retain(|k| k.public_key.fingerprint() != fingerprint);
        self.keys.len() < initial_len
    }

    /// Check if a public key is authorized
    pub fn is_authorized(&self, public_key: &PublicKey) -> bool {
        let fingerprint = public_key.fingerprint();
        self.keys
            .iter()
            .any(|k| k.public_key.fingerprint() == fingerprint)
    }

    /// Get a key by fingerprint
    pub fn get_key_by_fingerprint(&self, fingerprint: &str) -> Option<&AuthorizedKey> {
        self.keys
            .iter()
            .find(|k| k.public_key.fingerprint() == fingerprint)
    }

    /// List all keys
    pub fn list_keys(&self) -> &[AuthorizedKey] {
        &self.keys
    }

    /// Get the number of authorized keys
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Check if there are no authorized keys
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

impl Default for AuthorizedKeys {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{KeyPair, KeyType};
    use tempfile::TempDir;

    #[test]
    fn test_authorized_key_parsing() {
        // Test simple key
        let line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl user@host";
        let key = AuthorizedKey::from_line(line).unwrap();
        assert_eq!(key.comment.as_deref(), Some("user@host"));
        assert!(key.options.is_empty());

        // Test key with options
        let line = "no-port-forwarding,command=\"/usr/bin/rsync\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl rsync-only";
        let key = AuthorizedKey::from_line(line).unwrap();
        assert_eq!(key.comment.as_deref(), Some("rsync-only"));
        assert_eq!(key.options.len(), 2);
        assert!(key.options.contains(&"no-port-forwarding".to_string()));
    }

    #[test]
    fn test_authorized_key_to_line() {
        let key_pair = KeyPair::generate(KeyType::Ed25519).unwrap();
        let auth_key = AuthorizedKey {
            public_key: key_pair.public_key(),
            comment: Some("test@example.com".to_string()),
            options: vec!["no-x11-forwarding".to_string()],
        };

        let line = auth_key.to_line();
        assert!(line.starts_with("no-x11-forwarding"));
        assert!(line.contains("ssh-ed25519"));
        assert!(line.ends_with("test@example.com"));
    }

    #[tokio::test]
    async fn test_authorized_keys_save_load() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("authorized_keys");

        // Create keys
        let key1 = KeyPair::generate(KeyType::Ed25519).unwrap();
        let key2 = KeyPair::generate(KeyType::Ed25519).unwrap();

        let mut auth_keys = AuthorizedKeys::new();
        auth_keys.add_key(AuthorizedKey {
            public_key: key1.public_key(),
            comment: Some("key1".to_string()),
            options: vec![],
        });
        auth_keys.add_key(AuthorizedKey {
            public_key: key2.public_key(),
            comment: Some("key2".to_string()),
            options: vec!["no-agent-forwarding".to_string()],
        });

        // Save
        auth_keys.save_to_file(&auth_file).await.unwrap();
        assert!(auth_file.exists());

        // Load
        let loaded = AuthorizedKeys::load_from_file(&auth_file).await.unwrap();
        assert_eq!(loaded.len(), 2);

        // Verify keys
        assert!(loaded.is_authorized(&key1.public_key()));
        assert!(loaded.is_authorized(&key2.public_key()));

        // Check options preserved
        let key2_loaded = loaded
            .get_key_by_fingerprint(&key2.public_key().fingerprint())
            .unwrap();
        assert_eq!(key2_loaded.options, vec!["no-agent-forwarding"]);
    }

    #[test]
    fn test_remove_key() {
        let key1 = KeyPair::generate(KeyType::Ed25519).unwrap();
        let key2 = KeyPair::generate(KeyType::Ed25519).unwrap();

        let mut auth_keys = AuthorizedKeys::new();
        auth_keys.add_key(AuthorizedKey {
            public_key: key1.public_key(),
            comment: None,
            options: vec![],
        });
        auth_keys.add_key(AuthorizedKey {
            public_key: key2.public_key(),
            comment: None,
            options: vec![],
        });

        assert_eq!(auth_keys.len(), 2);

        // Remove key1
        let removed = auth_keys.remove_key_by_fingerprint(&key1.public_key().fingerprint());
        assert!(removed);
        assert_eq!(auth_keys.len(), 1);
        assert!(!auth_keys.is_authorized(&key1.public_key()));
        assert!(auth_keys.is_authorized(&key2.public_key()));
    }
}
