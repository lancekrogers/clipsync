//! Read-only adoption validation and byte-preserving adopt.

use crate::config::resolution::{
    is_general_ssh_authorized_keys, ConfigProvenance, ContentFingerprint,
};
use crate::config::Config;
use std::fs;
use std::path::{Path, PathBuf};

use super::error::{BootstrapError, BootstrapErrorCode};
use super::fsutil::{reject_symlink, SetupLock};
use super::identity::verify_identity_readable;
use super::journal::{write_journal, JournalStep, SetupJournal, SetupStepId, StepStatus};
use super::paths::BootstrapPaths;

/// Read-only adoption inspection (no writes).
#[derive(Debug, Clone)]
pub struct AdoptionReport {
    pub config_path: PathBuf,
    pub provenance_hint: Option<ConfigProvenance>,
    pub identity_path: PathBuf,
    pub identity_ok: bool,
    pub auth_path: PathBuf,
    pub auth_ok: bool,
    pub history_db: PathBuf,
    pub history_ok: bool,
    pub warnings: Vec<String>,
    pub blocked: Option<BootstrapErrorCode>,
}

/// Result of fresh or adopt bootstrap.
#[derive(Debug, Clone)]
pub struct BootstrapOutcome {
    pub config_path: PathBuf,
    pub identity_path: PathBuf,
    pub auth_path: PathBuf,
    pub journal_path: PathBuf,
    pub reused_identity: bool,
    pub mode: BootstrapMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapMode {
    Fresh,
    Adopt,
}

/// Validate an existing config for adoption without mutating identity/auth/history bytes.
pub fn validate_adoption(
    paths: &BootstrapPaths,
    config_path: &Path,
) -> Result<AdoptionReport, BootstrapError> {
    reject_symlink(config_path)?;
    let meta = fs::symlink_metadata(config_path)?;
    if !meta.file_type().is_file() {
        return Err(BootstrapError::AdoptionFailed(format!(
            "config is not a regular file: {}",
            config_path.display()
        )));
    }

    let contents = fs::read_to_string(config_path)?;
    let config = Config::parse_for_resolve(&contents)?;

    let mut warnings = Vec::new();
    let mut blocked = None;

    let identity_path = config.auth.ssh_key.clone();
    let identity_ok = match verify_identity_readable(&identity_path) {
        Ok(()) => true,
        Err(e) => {
            warnings.push(e.to_string());
            blocked = Some(BootstrapErrorCode::IdentityUnreadable);
            false
        }
    };

    let auth_path = config.auth.authorized_keys.clone();
    let auth_ok = if is_general_ssh_authorized_keys(&auth_path) {
        blocked = Some(BootstrapErrorCode::AuthFileIsGeneralSsh);
        warnings.push(format!("AuthFileIsGeneralSsh: {}", auth_path.display()));
        false
    } else {
        true
    };

    let history_db = config.clipboard.history_db.clone();
    let mut history_ok = true;
    if history_db.exists() {
        let key_path = resolve_history_key_path(paths, &config);
        if !key_path.exists() {
            history_ok = false;
            blocked = Some(BootstrapErrorCode::HistoryKeyMissingForExistingDb);
            warnings.push(format!(
                "History key missing for existing DB ({})",
                history_db.display()
            ));
        }
    }

    if identity_path.file_name().and_then(|s| s.to_str()) == Some("id_ed25519") {
        warnings.push(
            "Identity is general id_ed25519; dedicated id_ed25519_clipsync preferred (warn-only)"
                .into(),
        );
    }

    Ok(AdoptionReport {
        config_path: config_path.to_path_buf(),
        provenance_hint: None,
        identity_path,
        identity_ok,
        auth_path,
        auth_ok,
        history_db,
        history_ok,
        warnings,
        blocked,
    })
}

/// Adopt an existing valid config: journal selection only; preserve all asset bytes.
pub fn adopt_existing(
    paths: &BootstrapPaths,
    config_path: &Path,
) -> Result<BootstrapOutcome, BootstrapError> {
    let _lock = SetupLock::try_acquire(
        &paths.setup_lock_path(),
        &paths.setup_state_dir(),
        &paths.home,
    )?;

    let report = validate_adoption(paths, config_path)?;
    if let Some(code) = report.blocked {
        return Err(match code {
            BootstrapErrorCode::AuthFileIsGeneralSsh => {
                BootstrapError::AuthFileIsGeneralSsh(report.auth_path)
            }
            BootstrapErrorCode::IdentityUnreadable => {
                BootstrapError::IdentityUnreadable(report.identity_path, "adoption blocked".into())
            }
            BootstrapErrorCode::HistoryKeyMissingForExistingDb => {
                BootstrapError::HistoryKeyMissingForExistingDb(report.history_db)
            }
            other => BootstrapError::AdoptionFailed(format!("{:?}", other)),
        });
    }

    let before = snapshot_bytes(&[&report.identity_path, &report.auth_path, &report.history_db])?;

    let journal = SetupJournal {
        version: 1,
        mode: "adopt".into(),
        steps: vec![JournalStep {
            id: SetupStepId::Inspect,
            status: StepStatus::Committed,
            path: Some(config_path.to_path_buf()),
            fingerprint: ContentFingerprint::of_file(config_path)
                .ok()
                .map(|f| f.sha256_hex),
        }],
    };
    write_journal(paths, &journal)?;

    let after = snapshot_bytes(&[&report.identity_path, &report.auth_path, &report.history_db])?;
    if before != after {
        return Err(BootstrapError::AdoptionFailed(
            "asset bytes changed during adopt (unexpected)".into(),
        ));
    }

    Ok(BootstrapOutcome {
        config_path: config_path.to_path_buf(),
        identity_path: report.identity_path,
        auth_path: report.auth_path,
        journal_path: paths.journal_path(),
        reused_identity: true,
        mode: BootstrapMode::Adopt,
    })
}

/// Resolve history key for adoption: explicit config path, else historical platform default.
pub(super) fn resolve_history_key_path(paths: &BootstrapPaths, config: &Config) -> PathBuf {
    if let Some(ref p) = config.clipboard.history_key {
        return p.clone();
    }
    paths.historical_default_history_key()
}

fn snapshot_bytes(paths: &[&Path]) -> Result<Vec<(PathBuf, Option<Vec<u8>>)>, BootstrapError> {
    let mut out = Vec::new();
    for p in paths {
        let bytes = if p.exists() { Some(fs::read(p)?) } else { None };
        out.push((p.to_path_buf(), bytes));
    }
    Ok(out)
}
