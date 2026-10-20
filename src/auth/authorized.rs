//! SSH authorized_keys file management with atomic, flock-guarded updates.

use crate::auth::{AuthError, PublicKey};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::AsyncBufReadExt;

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

    /// Load from file (async). Fail-closed on unreadable paths (except missing → empty).
    pub async fn load_from_file(path: &Path) -> Result<Self, AuthError> {
        let file = match tokio::fs::File::open(path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(e) => return Err(e.into()),
        };
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

    /// Synchronous load for flock-guarded RMW inside `spawn_blocking`.
    pub fn load_from_file_sync(path: &Path) -> Result<Self, AuthError> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(e) => return Err(e.into()),
        };
        let mut keys = Vec::new();
        for (line_num, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            match AuthorizedKey::from_line(line) {
                Ok(key) => keys.push(key),
                Err(e) => {
                    eprintln!(
                        "Warning: Skipping invalid key at line {}: {}",
                        line_num + 1,
                        e
                    );
                }
            }
        }
        Ok(Self { keys })
    }

    fn lock_path(path: &Path) -> PathBuf {
        path.with_extension("lock")
    }

    /// Legacy predictable `path.tmp` (never used for writes — symlink/mode hazard).
    fn legacy_temp_path(path: &Path) -> PathBuf {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        PathBuf::from(tmp)
    }

    /// Unique sibling temp: `path.<uuid>.tmp` (create_new + O_NOFOLLOW).
    fn owned_temp_path(path: &Path) -> PathBuf {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".");
        tmp.push(uuid::Uuid::new_v4().as_hyphenated().to_string());
        tmp.push(".tmp");
        PathBuf::from(tmp)
    }

    /// Non-blocking flock with timeout. Symlink-safe (O_NOFOLLOW) lock sidecar, mode 0600.
    fn acquire_file_lock(path: &Path, timeout: Duration) -> Result<std::fs::File, AuthError> {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::io::AsRawFd;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock_path = Self::lock_path(path);
        let start = Instant::now();
        loop {
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&lock_path)?;
            let rc = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc == 0 {
                return Ok(lock);
            }
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::WouldBlock
                && err.raw_os_error() != Some(libc::EWOULDBLOCK)
                && err.raw_os_error() != Some(libc::EAGAIN)
            {
                return Err(AuthError::Io(err));
            }
            if start.elapsed() >= timeout {
                return Err(AuthError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "authorized_keys flock timed out",
                )));
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    fn sync_parent_dir(path: &Path) -> Result<(), AuthError> {
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        // Empty parent means cwd; still fsync it for durability confirmation.
        let dir_path = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        let dir = std::fs::File::open(dir_path).map_err(|e| {
            AuthError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "authorization file was published at {} but durability could not be confirmed (open parent): {e}",
                    path.display()
                ),
            ))
        })?;
        dir.sync_all().map_err(|e| {
            AuthError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "authorization file was published at {} but durability could not be confirmed: {e}",
                    path.display()
                ),
            ))
        })?;
        Ok(())
    }

    /// Atomic write via shared sync path (`spawn_blocking` — no async/blocking deadlock).
    pub async fn save_to_file(&self, path: &Path) -> Result<(), AuthError> {
        let keys = self.clone_keys();
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || keys.save_to_file_sync(&path, || {}))
            .await
            .map_err(|e| AuthError::Io(std::io::Error::other(format!("auth save join: {e}"))))?
    }

    /// Atomic publish: unique create_new temp (0600, O_NOFOLLOW) → fsync → rename →
    /// `after_publish` (commit point) → parent directory fsync.
    ///
    /// Cleanup removes only the owned temp. Legacy `path.tmp` is never opened.
    /// `after_publish` runs immediately after successful rename and before directory
    /// fsync so a post-publish durability error cannot look like "no write".
    fn save_to_file_sync<F>(&self, path: &Path, after_publish: F) -> Result<(), AuthError>
    where
        F: FnOnce(),
    {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let tmp = Self::owned_temp_path(path);
        // Track only a temp we successfully create_new'd; clear after rename so a
        // post-publish dirsync error never unlinks the published authorization file.
        let mut owned_tmp: Option<PathBuf> = None;

        let result = self.write_atomic_owned(path, &tmp, &mut owned_tmp, after_publish);
        if result.is_err() {
            if let Some(stale) = owned_tmp.take() {
                let _ = std::fs::remove_file(&stale);
            }
        }
        result
    }

    fn write_atomic_owned<F>(
        &self,
        path: &Path,
        tmp: &Path,
        owned_tmp: &mut Option<PathBuf>,
        after_publish: F,
    ) -> Result<(), AuthError>
    where
        F: FnOnce(),
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(tmp)?;
        *owned_tmp = Some(tmp.to_path_buf());

        writeln!(file, "# ClipSync authorized keys file")?;
        writeln!(
            file,
            "# This file contains public keys authorized to connect to this ClipSync instance"
        )?;
        writeln!(file)?;
        for key in &self.keys {
            writeln!(file, "{}", key.to_line())?;
        }
        file.flush()?;
        file.sync_all()?;
        drop(file);

        std::fs::rename(tmp, path)?;
        *owned_tmp = None;
        // PUBLISH: name is visible. Commit hooks must run here — before dir fsync —
        // so cancel cannot reopen the gate / claim no-write if durability sync fails.
        after_publish();
        Self::sync_parent_dir(path)?;
        Ok(())
    }

    /// Cross-process locked read-modify-write.
    ///
    /// Entire flock + load + mutate + atomic save runs inside `spawn_blocking` so a
    /// current-thread Tokio runtime cannot deadlock holding flock across `.await`.
    pub async fn update_file<F, T>(path: &Path, mutate: F) -> Result<T, AuthError>
    where
        F: FnOnce(&mut AuthorizedKeys) -> Result<T, AuthError> + Send + 'static,
        T: Send + 'static,
    {
        Self::update_file_with_hooks(path, mutate, || {}, || {}).await
    }

    /// Like [`update_file`], with hooks still under the flock:
    /// - `mutate` runs after load
    /// - `after_save` runs at PUBLISH (after rename, before directory fsync)
    /// - `on_save_fail` runs if save returns Err after mutate; if publish already
    ///   happened the commit gate is already COMMITTED (`abort_commit_attempt` no-ops)
    pub async fn update_file_with_hooks<F, A, E, T>(
        path: &Path,
        mutate: F,
        after_save: A,
        on_save_fail: E,
    ) -> Result<T, AuthError>
    where
        F: FnOnce(&mut AuthorizedKeys) -> Result<T, AuthError> + Send + 'static,
        A: FnOnce() + Send + 'static,
        E: FnOnce() + Send + 'static,
        T: Send + 'static,
    {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let _lock = Self::acquire_file_lock(&path, Duration::from_secs(5))?;
            let mut keys = Self::load_from_file_sync(&path)?;
            let result = mutate(&mut keys)?;
            match keys.save_to_file_sync(&path, after_save) {
                Ok(()) => Ok(result),
                Err(e) => {
                    on_save_fail();
                    Err(e)
                }
            }
        })
        .await
        .map_err(|e| AuthError::Io(std::io::Error::other(format!("auth update join: {e}"))))?
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

    /// Deep copy used when refreshing an in-memory authenticator cache after
    /// a locked file update.
    pub fn clone_keys(&self) -> Self {
        Self {
            keys: self.keys.clone(),
        }
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

    #[tokio::test]
    async fn atomic_save_preserves_prior_on_rename_target() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("authorized_keys");
        let key1 = KeyPair::generate(KeyType::Ed25519).unwrap();
        let mut keys = AuthorizedKeys::new();
        keys.add_key(AuthorizedKey {
            public_key: key1.public_key(),
            comment: Some("keep".into()),
            options: vec![],
        });
        keys.save_to_file(&auth_file).await.unwrap();
        let key2 = KeyPair::generate(KeyType::Ed25519).unwrap();
        keys.add_key(AuthorizedKey {
            public_key: key2.public_key(),
            comment: Some("added".into()),
            options: vec![],
        });
        keys.save_to_file(&auth_file).await.unwrap();
        let loaded = AuthorizedKeys::load_from_file(&auth_file).await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(!AuthorizedKeys::legacy_temp_path(&auth_file).exists());
        let leftover_owned: Vec<_> = std::fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let s = name.to_string_lossy();
                s.starts_with("authorized_keys.") && s.ends_with(".tmp")
            })
            .collect();
        assert!(
            leftover_owned.is_empty(),
            "owned UUID temps must be cleaned or renamed away: {leftover_owned:?}"
        );
    }

    #[tokio::test]
    async fn legacy_tmp_symlink_victim_untouched() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("authorized_keys");
        let victim = temp_dir.path().join("victim_secret");
        std::fs::write(&victim, b"DO_NOT_CLOBBER").unwrap();
        let legacy_tmp = AuthorizedKeys::legacy_temp_path(&auth_file);
        std::os::unix::fs::symlink(&victim, &legacy_tmp).unwrap();

        let key = KeyPair::generate(KeyType::Ed25519).unwrap();
        let mut keys = AuthorizedKeys::new();
        keys.add_key(AuthorizedKey {
            public_key: key.public_key(),
            comment: Some("safe".into()),
            options: vec![],
        });
        keys.save_to_file(&auth_file).await.unwrap();

        assert_eq!(std::fs::read(&victim).unwrap(), b"DO_NOT_CLOBBER");
        assert!(std::fs::symlink_metadata(&legacy_tmp)
            .unwrap()
            .file_type()
            .is_symlink());
        let loaded = AuthorizedKeys::load_from_file(&auth_file).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded.is_authorized(&key.public_key()));
    }

    #[tokio::test]
    async fn mode_600_ignores_loose_legacy_tmp() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("authorized_keys");
        let legacy_tmp = AuthorizedKeys::legacy_temp_path(&auth_file);
        std::fs::write(&legacy_tmp, b"LOOSE_PREEXISTING").unwrap();
        let mut loose = std::fs::metadata(&legacy_tmp).unwrap().permissions();
        loose.set_mode(0o666);
        std::fs::set_permissions(&legacy_tmp, loose).unwrap();

        let key = KeyPair::generate(KeyType::Ed25519).unwrap();
        let mut keys = AuthorizedKeys::new();
        keys.add_key(AuthorizedKey {
            public_key: key.public_key(),
            comment: Some("perm".into()),
            options: vec![],
        });
        keys.save_to_file(&auth_file).await.unwrap();

        let mode = std::fs::metadata(&auth_file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "published authorized_keys must be mode 0600");
        assert_eq!(std::fs::read(&legacy_tmp).unwrap(), b"LOOSE_PREEXISTING");
        let legacy_mode = std::fs::metadata(&legacy_tmp).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            legacy_mode, 0o666,
            "legacy .tmp must not be reused/truncated"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rename_failure_preserves_old_trust_and_cleans_owned_temp() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("authorized_keys");
        let key1 = KeyPair::generate(KeyType::Ed25519).unwrap();
        let mut keys = AuthorizedKeys::new();
        keys.add_key(AuthorizedKey {
            public_key: key1.public_key(),
            comment: Some("keep".into()),
            options: vec![],
        });
        keys.save_to_file_sync(&auth_file, || {}).unwrap();
        let original = std::fs::read(&auth_file).unwrap();

        // Immutable destination → rename-over fails; prior trust must remain.
        let status = std::process::Command::new("chflags")
            .args(["uchg"])
            .arg(&auth_file)
            .status()
            .expect("chflags");
        assert!(status.success());

        let key2 = KeyPair::generate(KeyType::Ed25519).unwrap();
        keys.add_key(AuthorizedKey {
            public_key: key2.public_key(),
            comment: Some("new".into()),
            options: vec![],
        });
        let err = keys.save_to_file_sync(&auth_file, || {}).unwrap_err();
        assert!(
            matches!(err, AuthError::Io(_)),
            "expected rename IO error, got {err:?}"
        );

        let _ = std::process::Command::new("chflags")
            .args(["nouchg"])
            .arg(&auth_file)
            .status();

        assert_eq!(std::fs::read(&auth_file).unwrap(), original);
        let loaded = AuthorizedKeys::load_from_file_sync(&auth_file).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded.is_authorized(&key1.public_key()));
        assert!(!loaded.is_authorized(&key2.public_key()));

        let leftover_owned: Vec<_> = std::fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let s = name.to_string_lossy();
                s.starts_with("authorized_keys.") && s.ends_with(".tmp")
            })
            .collect();
        assert!(
            leftover_owned.is_empty(),
            "owned temp must be removed after rename failure: {leftover_owned:?}"
        );
    }

    #[tokio::test]
    async fn after_save_hook_runs_at_publish() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };

        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("authorized_keys");
        let published = Arc::new(AtomicBool::new(false));
        let flag = published.clone();
        let hook_path = auth_file.clone();
        let key = KeyPair::generate(KeyType::Ed25519).unwrap();
        let pk = key.public_key();

        AuthorizedKeys::update_file_with_hooks(
            &auth_file,
            move |keys| {
                keys.add_key(AuthorizedKey {
                    public_key: pk,
                    comment: Some("hook".into()),
                    options: vec![],
                });
                Ok(())
            },
            move || {
                // Must see published content when after_save runs (post-rename).
                let loaded = AuthorizedKeys::load_from_file_sync(&hook_path).unwrap();
                assert_eq!(loaded.len(), 1);
                flag.store(true, Ordering::SeqCst);
            },
            || panic!("on_save_fail must not run on success"),
        )
        .await
        .unwrap();

        assert!(published.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn update_file_revokes_one_key_preserves_others() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("authorized_keys");
        let keep = KeyPair::generate(KeyType::Ed25519).unwrap();
        let drop = KeyPair::generate(KeyType::Ed25519).unwrap();
        let keep_pk = keep.public_key();
        let drop_pk = drop.public_key();
        AuthorizedKeys::update_file(&auth_file, move |keys| {
            keys.add_key(AuthorizedKey {
                public_key: keep_pk,
                comment: Some("keep".into()),
                options: vec![],
            });
            keys.add_key(AuthorizedKey {
                public_key: drop_pk,
                comment: Some("drop".into()),
                options: vec![],
            });
            Ok(())
        })
        .await
        .unwrap();

        let drop_fp = drop.public_key().fingerprint();
        let removed = AuthorizedKeys::update_file(&auth_file, move |keys| {
            Ok(keys.remove_key_by_fingerprint(&drop_fp))
        })
        .await
        .unwrap();
        assert!(removed);
        let loaded = AuthorizedKeys::load_from_file(&auth_file).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded.is_authorized(&keep.public_key()));
        assert!(!loaded.is_authorized(&drop.public_key()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn update_file_works_on_current_thread_runtime() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("authorized_keys");
        let key = KeyPair::generate(KeyType::Ed25519).unwrap();
        let pk = key.public_key();
        AuthorizedKeys::update_file(&auth_file, move |keys| {
            keys.add_key(AuthorizedKey {
                public_key: pk,
                comment: Some("ct".into()),
                options: vec![],
            });
            Ok(())
        })
        .await
        .unwrap();
        let loaded = AuthorizedKeys::load_from_file_sync(&auth_file).unwrap();
        assert_eq!(loaded.len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_current_thread_updates() {
        let temp_dir = TempDir::new().unwrap();
        let auth_file = temp_dir.path().join("authorized_keys");
        let key_a = KeyPair::generate(KeyType::Ed25519).unwrap();
        let key_b = KeyPair::generate(KeyType::Ed25519).unwrap();
        let pk_a = key_a.public_key();
        let pk_b = key_b.public_key();
        let path_a = auth_file.clone();
        let path_b = auth_file.clone();

        let (r1, r2) = tokio::join!(
            AuthorizedKeys::update_file(&path_a, move |keys| {
                keys.add_key(AuthorizedKey {
                    public_key: pk_a,
                    comment: Some("a".into()),
                    options: vec![],
                });
                Ok(())
            }),
            AuthorizedKeys::update_file(&path_b, move |keys| {
                keys.add_key(AuthorizedKey {
                    public_key: pk_b,
                    comment: Some("b".into()),
                    options: vec![],
                });
                Ok(())
            }),
        );
        r1.unwrap();
        r2.unwrap();

        let loaded = AuthorizedKeys::load_from_file_sync(&auth_file).unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.is_authorized(&key_a.public_key()));
        assert!(loaded.is_authorized(&key_b.public_key()));
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
