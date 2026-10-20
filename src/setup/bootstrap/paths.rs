//! Fixture-friendly roots for bootstrap and history-key resolution.

use crate::config::resolution::ResolvePaths;
use std::path::PathBuf;

use super::error::BootstrapError;

/// Explicit roots and env overlays for bootstrap/resolution (fixture-friendly).
#[derive(Debug, Clone)]
pub struct BootstrapPaths {
    pub home: PathBuf,
    pub xdg_config_home: Option<PathBuf>,
    pub xdg_state_home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    /// When set, treated as `CLIPSYNC_CONFIG` for resolution.
    pub clipsync_config: Option<PathBuf>,
}

impl BootstrapPaths {
    pub fn from_process_env() -> Result<Self, BootstrapError> {
        let home = dirs::home_dir().ok_or_else(|| {
            BootstrapError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not determine home directory",
            ))
        })?;
        Ok(Self {
            home,
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            xdg_state_home: std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
            xdg_data_home: std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
            clipsync_config: std::env::var_os("CLIPSYNC_CONFIG").map(PathBuf::from),
        })
    }

    /// Isolated fixture: `home/.config`, `home/.local/{state,share}` — no process env writes.
    pub fn for_fixture(root: impl Into<PathBuf>) -> Self {
        let home = root.into();
        Self {
            xdg_config_home: Some(home.join(".config")),
            xdg_state_home: Some(home.join(".local/state")),
            xdg_data_home: Some(home.join(".local/share")),
            clipsync_config: None,
            home,
        }
    }

    pub fn with_clipsync_config(mut self, path: Option<PathBuf>) -> Self {
        self.clipsync_config = path;
        self
    }

    pub fn to_resolve_paths(&self) -> ResolvePaths {
        ResolvePaths {
            home: self.home.clone(),
            xdg_config_home: self.xdg_config_home.clone(),
            clipsync_config: self.clipsync_config.clone(),
        }
    }

    pub fn config_dir(&self) -> PathBuf {
        self.xdg_config_home
            .clone()
            .unwrap_or_else(|| self.home.join(".config"))
            .join("clipsync")
    }

    pub fn config_toml(&self) -> PathBuf {
        self.config_dir().join("config.toml")
    }

    pub fn authorized_devices(&self) -> PathBuf {
        self.config_dir().join("authorized_devices")
    }

    pub fn ssh_dir(&self) -> PathBuf {
        self.home.join(".ssh")
    }

    pub fn dedicated_identity(&self) -> PathBuf {
        self.ssh_dir().join("id_ed25519_clipsync")
    }

    pub fn dedicated_identity_pub(&self) -> PathBuf {
        self.ssh_dir().join("id_ed25519_clipsync.pub")
    }

    pub fn history_db(&self) -> PathBuf {
        self.xdg_data_home
            .clone()
            .unwrap_or_else(|| self.home.join(".local/share"))
            .join("clipsync")
            .join("history.db")
    }

    /// Fresh-setup history key: always `config_dir/history.key` (root amendment).
    pub fn fresh_history_key(&self) -> PathBuf {
        self.config_dir().join("history.key")
    }

    /// Historical platform default when `clipboard.history_key` is absent.
    ///
    /// Matches `Encryptor::get_key_file_path`: on macOS this is Application Support
    /// even when the selected config lives under XDG; on Linux it follows XDG/config.
    /// Uses fixture overlays — never process `HOME` in tests.
    pub fn historical_default_history_key(&self) -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            self.home
                .join("Library/Application Support/clipsync/history.key")
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.config_dir().join("history.key")
        }
    }

    pub fn setup_state_dir(&self) -> PathBuf {
        self.xdg_state_home
            .clone()
            .unwrap_or_else(|| self.home.join(".local/state"))
            .join("clipsync")
            .join("setup")
    }

    pub fn journal_path(&self) -> PathBuf {
        self.setup_state_dir().join("journal.json")
    }

    pub fn setup_lock_path(&self) -> PathBuf {
        self.setup_state_dir().join("setup.lock")
    }
}
