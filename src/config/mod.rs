//! Configuration management for ClipSync
//!
//! This module handles loading, validating, and managing configuration
//! for the ClipSync service.

pub mod resolution;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Configuration errors
#[derive(Debug, Error)]
pub enum ConfigError {
    /// IO error reading config file
    #[error("Failed to read config file: {0}")]
    Io(#[from] std::io::Error),

    /// TOML parsing error
    #[error("Failed to parse TOML: {0}")]
    Toml(#[from] toml::de::Error),

    /// Validation error
    #[error("Config validation failed: {0}")]
    Validation(String),

    /// SSH key not found
    #[error("SSH key not found at path: {0}")]
    SshKeyNotFound(PathBuf),

    /// Invalid size value
    #[error("Invalid size value: {0}")]
    InvalidSize(String),

    /// Explicit `--config` or `CLIPSYNC_CONFIG` path missing/unreadable (D02).
    #[error("Missing explicit config at path: {path}")]
    MissingExplicitConfig { path: PathBuf },

    /// Multiple distinct config files require user choice (D02).
    #[error("Config conflict across paths: {paths:?}")]
    ConfigConflict { paths: Vec<PathBuf> },

    /// Other resolution failure
    #[error("Config resolution failed: {0}")]
    Resolution(String),
}

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Node ID (generated if not specified)
    #[serde(default = "generate_node_id")]
    pub node_id: uuid::Uuid,
    /// Network address to listen on
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,

    /// Name advertised for mDNS discovery
    #[serde(default = "default_advertise_name")]
    pub advertise_name: String,

    /// Authentication configuration
    #[serde(default)]
    pub auth: AuthConfig,

    /// Clipboard configuration
    #[serde(default)]
    pub clipboard: ClipboardConfig,

    /// Hotkey configuration
    #[serde(default)]
    pub hotkeys: HotkeyConfig,

    /// Security configuration
    #[serde(default)]
    pub security: SecurityConfig,

    /// Log level
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

/// Authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Path to SSH private key for peer authentication
    #[serde(default = "default_ssh_key")]
    pub ssh_key: PathBuf,

    /// Path to authorized keys file
    #[serde(default = "default_authorized_keys")]
    pub authorized_keys: PathBuf,
}

impl AuthConfig {
    /// Get the public key path based on the private key path
    pub fn get_public_key_path(&self) -> PathBuf {
        let mut pub_path = self.ssh_key.clone();
        let filename = pub_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        pub_path.set_file_name(format!("{}.pub", filename));
        pub_path
    }

    /// Load the public key content
    pub async fn load_public_key(&self) -> Result<String, ConfigError> {
        let pub_path = self.get_public_key_path();
        if !pub_path.exists() {
            return Err(ConfigError::SshKeyNotFound(pub_path));
        }

        let content = tokio::fs::read_to_string(&pub_path)
            .await
            .map_err(|e| ConfigError::Io(e))?;
        Ok(content.trim().to_string())
    }
}

/// Clipboard configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardConfig {
    /// Optional isolated key path; absent preserves the existing platform key location.
    #[serde(default)]
    pub history_key: Option<PathBuf>,
    /// Maximum payload size in bytes
    #[serde(default = "default_max_size")]
    pub max_size: usize,

    /// Whether to sync middle-click selection on Linux
    #[serde(default = "default_sync_primary")]
    pub sync_primary: bool,

    /// Number of clipboard items to keep in history
    #[serde(default = "default_history_size")]
    pub history_size: usize,

    /// Path to SQLite database for history
    #[serde(default = "default_history_db")]
    pub history_db: PathBuf,
}

/// Hotkey configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyConfig {
    /// Hotkey to toggle sync
    #[serde(default = "default_toggle_sync")]
    pub toggle_sync: String,

    /// Hotkey to show clipboard history
    #[serde(default = "default_show_history")]
    pub show_history: String,

    /// Hotkey to cycle to previous clipboard item
    #[serde(default = "default_cycle_prev")]
    pub cycle_prev: String,

    /// Hotkey to cycle to next clipboard item
    #[serde(default = "default_cycle_next")]
    pub cycle_next: String,
}

/// Security configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Encryption algorithm
    #[serde(default = "default_encryption")]
    pub encryption: String,

    /// Compression algorithm for large payloads
    #[serde(default = "default_compression")]
    pub compression: String,
}

// Default value functions
fn default_node_id() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

fn default_listen_addr() -> String {
    "0.0.0.0:8484".to_string()
}

fn default_advertise_name() -> String {
    let hostname = gethostname::gethostname().to_string_lossy().to_string();
    format!("{}-clipsync", hostname)
}

fn default_ssh_key() -> PathBuf {
    PathBuf::from("~/.ssh/id_ed25519")
}

fn default_authorized_keys() -> PathBuf {
    PathBuf::from("~/.config/clipsync/authorized_keys")
}

fn default_max_size() -> usize {
    5_242_880 // 5MB
}

fn default_sync_primary() -> bool {
    true
}

fn default_history_size() -> usize {
    20
}

fn default_history_db() -> PathBuf {
    PathBuf::from("~/.local/share/clipsync/history.db")
}

fn default_toggle_sync() -> String {
    #[cfg(target_os = "macos")]
    return "Ctrl+Shift+Cmd+C".to_string();

    #[cfg(not(target_os = "macos"))]
    return "Ctrl+Shift+Alt+C".to_string();
}

fn default_show_history() -> String {
    "Ctrl+Shift+V".to_string()
}

fn default_cycle_prev() -> String {
    "Ctrl+Shift+[".to_string()
}

fn default_cycle_next() -> String {
    "Ctrl+Shift+]".to_string()
}

fn default_encryption() -> String {
    "aes-256-gcm".to_string()
}

fn default_compression() -> String {
    "zstd".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

fn generate_node_id() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

// Default implementations
impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            ssh_key: default_ssh_key(),
            authorized_keys: default_authorized_keys(),
        }
    }
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            history_key: None,
            max_size: default_max_size(),
            sync_primary: default_sync_primary(),
            history_size: default_history_size(),
            history_db: default_history_db(),
        }
    }
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            toggle_sync: default_toggle_sync(),
            show_history: default_show_history(),
            cycle_prev: default_cycle_prev(),
            cycle_next: default_cycle_next(),
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            encryption: default_encryption(),
            compression: default_compression(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            node_id: generate_node_id(),
            listen_addr: default_listen_addr(),
            advertise_name: default_advertise_name(),
            auth: AuthConfig::default(),
            clipboard: ClipboardConfig::default(),
            hotkeys: HotkeyConfig::default(),
            security: SecurityConfig::default(),
            log_level: default_log_level(),
        }
    }
}

impl Config {
    /// Load configuration via the D02 canonical resolver.
    ///
    /// - Missing `CLIPSYNC_CONFIG` / unreadable env path → hard fail
    /// - Distinct XDG + legacy macOS configs → hard fail (`ConfigConflict`)
    /// - No files → legacy in-memory defaults (CLI compatibility); companion
    ///   must use [`resolution::ResolveOutcome::FreshEligible`] / bootstrap instead
    pub fn load() -> Result<Self, ConfigError> {
        let paths = resolution::ResolvePaths::from_process_env()?;
        resolution::resolve_config(&paths, None).into_config_for_cli()
    }

    /// Load configuration from a specific path.
    ///
    /// Missing/unreadable paths fail hard (preserves explicit `--config` behavior
    /// with D02 correction: no silent fallback).
    pub fn load_from_path(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_file() => {}
            Ok(_) => {
                return Err(ConfigError::MissingExplicitConfig {
                    path: path.to_path_buf(),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ConfigError::MissingExplicitConfig {
                    path: path.to_path_buf(),
                });
            }
            Err(e) => return Err(ConfigError::Io(e)),
        }
        let contents = std::fs::read_to_string(path)?;
        Self::from_toml(&contents)
    }

    /// Parse configuration from TOML string
    pub fn from_toml(toml_str: &str) -> Result<Self, ConfigError> {
        let mut config = Self::parse_for_resolve(toml_str)?;
        config.validate_ssh_key()?;
        Ok(config)
    }

    /// Parse + validate schema without requiring the SSH private key to exist.
    /// Used by the resolver and bootstrap soft-inspect paths.
    pub fn parse_for_resolve(toml_str: &str) -> Result<Self, ConfigError> {
        let mut config: Config = toml::from_str(toml_str)?;
        config.expand_paths();
        config.validate_config()?;
        Ok(config)
    }

    /// Expand tilde in paths
    pub(crate) fn expand_paths(&mut self) {
        self.auth.ssh_key = expand_path(&self.auth.ssh_key);
        self.auth.authorized_keys = expand_path(&self.auth.authorized_keys);
        self.clipboard.history_db = expand_path(&self.clipboard.history_db);
        self.clipboard.history_key = self.clipboard.history_key.as_ref().map(|p| expand_path(p));
    }

    /// Validate SSH key exists and is readable
    fn validate_ssh_key(&self) -> Result<(), ConfigError> {
        // Skip validation in tests
        #[cfg(test)]
        {
            return Ok(());
        }

        #[cfg(not(test))]
        {
            if !self.auth.ssh_key.exists() {
                return Err(ConfigError::SshKeyNotFound(self.auth.ssh_key.clone()));
            }

            // Check if readable
            std::fs::metadata(&self.auth.ssh_key)
                .map_err(|_| ConfigError::SshKeyNotFound(self.auth.ssh_key.clone()))?;

            Ok(())
        }
    }

    pub fn socket_addr(&self) -> Result<std::net::SocketAddr, ConfigError> {
        let address = if self.listen_addr.starts_with(':') {
            format!("0.0.0.0{}", self.listen_addr)
        } else {
            self.listen_addr.clone()
        };
        address
            .parse()
            .map_err(|e| ConfigError::Validation(format!("Invalid listen address: {e}")))
    }

    /// Validate configuration values
    fn validate_config(&self) -> Result<(), ConfigError> {
        if self.socket_addr()?.port() == 0 {
            return Err(ConfigError::Validation(
                "Use a nonzero listener port in daemon configuration".into(),
            ));
        }
        // Validate max_size range (1KB to 50MB)
        if self.clipboard.max_size < 1024 {
            return Err(ConfigError::Validation(
                "max_size must be at least 1024 bytes (1KB)".to_string(),
            ));
        }
        if self.clipboard.max_size > 52_428_800 {
            return Err(ConfigError::Validation(
                "max_size must not exceed 52428800 bytes (50MB)".to_string(),
            ));
        }

        // Validate history_size range (1 to 100)
        if self.clipboard.history_size < 1 {
            return Err(ConfigError::Validation(
                "history_size must be at least 1".to_string(),
            ));
        }
        if self.clipboard.history_size > 100 {
            return Err(ConfigError::Validation(
                "history_size must not exceed 100".to_string(),
            ));
        }

        Ok(())
    }

    /// Save configuration to default location
    pub fn save(&self) -> Result<(), ConfigError> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| {
                ConfigError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Could not find config directory",
                ))
            })?
            .join("clipsync");

        std::fs::create_dir_all(&config_dir)?;

        let config_path = config_dir.join("config.toml");
        let toml_string = toml::to_string_pretty(self).map_err(|e| {
            ConfigError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })?;

        std::fs::write(config_path, toml_string)?;

        Ok(())
    }

    /// Validate configuration file at given path
    pub async fn validate(path: &std::path::Path) -> Result<(), ConfigError> {
        // Try to load and validate the config
        match Self::load_from_path(path) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Load configuration with optional custom path (async wrapper).
    ///
    /// `Some(path)` is an explicit override: missing path → [`ConfigError::MissingExplicitConfig`].
    pub async fn load_config(config_path: Option<std::path::PathBuf>) -> Result<Self, ConfigError> {
        if let Some(path) = config_path {
            let paths = resolution::ResolvePaths::from_process_env()?;
            resolution::resolve_config(&paths, Some((path, resolution::ExplicitKind::Flag)))
                .into_config_for_cli()
        } else {
            Self::load()
        }
    }

    fn default_config_path() -> Result<std::path::PathBuf, ConfigError> {
        if let Ok(path) = std::env::var("CLIPSYNC_CONFIG") {
            return Ok(std::path::PathBuf::from(path));
        }

        let config_dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            std::path::PathBuf::from(xdg).join("clipsync")
        } else {
            dirs::config_dir()
                .ok_or_else(|| {
                    ConfigError::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "Could not find config directory",
                    ))
                })?
                .join("clipsync")
        };

        Ok(config_dir.join("config.toml"))
    }

    /// Generate example configuration file
    pub async fn generate_example_config(force: bool) -> Result<(), ConfigError> {
        Self::generate_example_config_at(&Self::default_config_path()?, force).await
    }

    /// Initialize a specific config without parsing the old file or requiring an identity.
    pub async fn generate_example_config_at(
        config_path: &Path,
        force: bool,
    ) -> Result<(), ConfigError> {
        let example_content = Self::generate_example();
        if let Some(parent) = config_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }

        if !force && config_path.exists() {
            return Err(ConfigError::Validation(
                "Config file already exists. Use --force to overwrite.".to_string(),
            ));
        }

        std::fs::write(config_path, example_content)?;
        Ok(())
    }

    /// Dedicated-identity template for companion fresh setup and `config init`.
    ///
    /// Uses `id_ed25519_clipsync`, ClipSync-only `authorized_devices`, and an
    /// explicit `clipboard.history_key` under the canonical config dir (root
    /// amendment). Does not expand tildes; callers expand on load.
    pub fn dedicated_identity_template() -> Self {
        Self {
            auth: AuthConfig {
                ssh_key: PathBuf::from("~/.ssh/id_ed25519_clipsync"),
                authorized_keys: PathBuf::from("~/.config/clipsync/authorized_devices"),
            },
            clipboard: ClipboardConfig {
                history_key: Some(PathBuf::from("~/.config/clipsync/history.key")),
                ..ClipboardConfig::default()
            },
            ..Self::default()
        }
    }

    /// Fixture-rooted fresh companion config (explicit history key under config dir).
    pub fn fresh_companion_template(
        ssh_key: PathBuf,
        authorized_devices: PathBuf,
        history_db: PathBuf,
        history_key: PathBuf,
    ) -> Self {
        Self {
            auth: AuthConfig {
                ssh_key,
                authorized_keys: authorized_devices,
            },
            clipboard: ClipboardConfig {
                history_db,
                history_key: Some(history_key),
                ..ClipboardConfig::default()
            },
            ..Self::default()
        }
    }

    /// Serialize a config to TOML safely (quoted paths/names).
    pub fn to_toml_string(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self)
            .map_err(|e| ConfigError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
    }

    /// Generate example configuration file (dedicated-identity template, D02).
    pub fn generate_example() -> String {
        let config = Self::dedicated_identity_template();
        let body = config
            .to_toml_string()
            .unwrap_or_else(|_| toml::to_string_pretty(&config).unwrap_or_default());
        format!(
            "# ClipSync Configuration File\n\
             # Location: ~/.config/clipsync/config.toml\n\
             # Dedicated identity + ClipSync-only authorization (D02)\n\
             # history_key is explicit under the config directory for fresh installs.\n\n\
             {body}"
        )
    }
}

/// Expand tilde in path
fn expand_path(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    let expanded = shellexpand::tilde(path_str.as_ref());
    PathBuf::from(expanded.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.listen_addr, "0.0.0.0:8484");
        assert_eq!(config.clipboard.max_size, 5_242_880);
        assert_eq!(config.clipboard.history_size, 20);
        assert!(config.clipboard.sync_primary);
    }

    #[test]
    fn test_load_from_toml() {
        let toml_str = r#"
            listen_addr = ":9999"
            advertise_name = "test-machine"

            [clipboard]
            max_size = 1_048_576
            history_size = 10
        "#;

        let config = Config::from_toml(toml_str).unwrap();
        assert_eq!(config.listen_addr, ":9999");
        assert_eq!(config.advertise_name, "test-machine");
        assert_eq!(config.clipboard.max_size, 1_048_576);
        assert_eq!(config.clipboard.history_size, 10);
    }

    #[test]
    fn test_validation_max_size() {
        let toml_str = r#"
            [clipboard]
            max_size = 100_000_000
        "#;

        let result = Config::from_toml(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_history_size() {
        let toml_str = r#"
            [clipboard]
            history_size = 200
        "#;

        let result = Config::from_toml(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let config = Config::default();
        let path = temp_dir.path().join("config.toml");
        std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
        let loaded = Config::load_from_path(&path).unwrap();
        assert_eq!(config.listen_addr, loaded.listen_addr);
    }

    #[test]
    fn test_generate_example() {
        let example = Config::generate_example();
        assert!(example.contains("ClipSync Configuration"));
        assert!(example.contains("max_size = 5242880"));
        assert!(example.contains("id_ed25519_clipsync"));
        assert!(example.contains("authorized_devices"));
        assert!(example.contains("history.key"));
        assert!(!example.contains("~/.ssh/id_ed25519\""));
    }

    #[test]
    fn dedicated_template_toml_quotes_special_chars() {
        let mut config = Config::dedicated_identity_template();
        config.advertise_name = r#"dev"with\slash"#.into();
        let body = config.to_toml_string().unwrap();
        let parsed: Config = toml::from_str(&body).unwrap();
        assert_eq!(parsed.advertise_name, config.advertise_name);
        assert!(parsed
            .auth
            .ssh_key
            .to_string_lossy()
            .contains("id_ed25519_clipsync"));
    }
}
