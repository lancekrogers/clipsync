//! Bootstrap error vocabulary (D02).

use crate::config::ConfigError;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BootstrapError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("Authentication error: {0}")]
    Auth(#[from] crate::auth::AuthError),

    #[error("Dedicated identity already exists at {0}")]
    IdentityExists(PathBuf),

    #[error("Identity unreadable or invalid at {0}: {1}")]
    IdentityUnreadable(PathBuf, String),

    #[error("Authorization file is general SSH authorized_keys: {0}")]
    AuthFileIsGeneralSsh(PathBuf),

    #[error("History key missing for existing DB at {0}")]
    HistoryKeyMissingForExistingDb(PathBuf),

    #[error("Refusing to clobber history key/DB at {0}")]
    HistoryKeyClobberBlocked(PathBuf),

    #[error("Symlink rejected at {0}")]
    SymlinkRejected(PathBuf),

    #[error("Config already exists at {0}")]
    ConfigExists(PathBuf),

    #[error("Not eligible for fresh setup: {0}")]
    NotFreshEligible(String),

    #[error("Adoption validation failed: {0}")]
    AdoptionFailed(String),

    #[error("Journal corrupt: {0}")]
    JournalCorrupt(String),

    #[error("Setup already in progress (journal lock held)")]
    SetupInProgress,

    #[error("Resolution conflict: choose a config first")]
    ConfigConflict,

    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapErrorCode {
    AuthFileIsGeneralSsh,
    IdentityUnreadable,
    HistoryKeyMissingForExistingDb,
    ConfigConflict,
    NotFreshEligible,
}
