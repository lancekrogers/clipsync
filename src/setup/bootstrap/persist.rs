//! Auth file and fresh config.toml publish.

use crate::config::resolution::{is_general_ssh_authorized_keys, ContentFingerprint};
use crate::config::Config;
use std::fs;
use std::path::{Path, PathBuf};

use super::error::BootstrapError;
use super::fsutil::{
    ensure_private_dir, publish_hard_link_no_clobber, reject_symlink, sync_dir, write_create_new,
};
use super::journal::{verify_fingerprint, SetupJournal, SetupStepId, StepStatus};
use super::paths::BootstrapPaths;

pub(super) fn ensure_auth_file(
    paths: &BootstrapPaths,
    journal: &SetupJournal,
) -> Result<PathBuf, BootstrapError> {
    let config_dir = paths.config_dir();
    ensure_private_dir(&config_dir, 0o700, &paths.home)?;
    let auth = paths.authorized_devices();

    if is_general_ssh_authorized_keys(&auth) {
        return Err(BootstrapError::AuthFileIsGeneralSsh(auth));
    }

    if let Some(step) = journal.step(SetupStepId::AuthFile) {
        if step.status == StepStatus::Committed {
            let path = step.path.clone().unwrap_or_else(|| auth.clone());
            reject_symlink(&path)?;
            if !path.is_file() {
                return Err(BootstrapError::JournalCorrupt(format!(
                    "committed auth file missing: {}",
                    path.display()
                )));
            }
            if let Some(ref fp) = step.fingerprint {
                verify_fingerprint(&path, fp)?;
            }
            return Ok(path);
        }
    }

    match fs::symlink_metadata(&auth) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(BootstrapError::SymlinkRejected(auth));
        }
        Ok(meta) if meta.file_type().is_file() => {
            // Present without our committed step: treat as foreign — only OK if empty
            // and we are creating fresh layout after identity. Prefer fail-closed for
            // non-empty foreign auth; empty ClipSync auth file may be left from prior
            // interrupted run of *this* incomplete journal (Inspect/Identity only).
            let meta_len = meta.len();
            if meta_len == 0 {
                return Ok(auth);
            }
            return Err(BootstrapError::Other(format!(
                "authorization file already exists without journal proof: {}",
                auth.display()
            )));
        }
        Ok(_) => {
            return Err(BootstrapError::Other(format!(
                "auth path exists but is not a file: {}",
                auth.display()
            )));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(BootstrapError::Io(e)),
    }

    write_create_new(&auth, b"", 0o600)?;
    Ok(auth)
}

pub(super) fn write_fresh_config_atomic(
    paths: &BootstrapPaths,
    journal: &SetupJournal,
) -> Result<PathBuf, BootstrapError> {
    let config_dir = paths.config_dir();
    ensure_private_dir(&config_dir, 0o700, &paths.home)?;
    reject_symlink(&config_dir)?;
    let config_path = paths.config_toml();

    if let Some(step) = journal.step(SetupStepId::ConfigWrite) {
        if step.status == StepStatus::Committed {
            let path = step.path.clone().unwrap_or_else(|| config_path.clone());
            reject_symlink(&path)?;
            if !path.is_file() {
                return Err(BootstrapError::JournalCorrupt(format!(
                    "committed config missing: {}",
                    path.display()
                )));
            }
            if let Some(ref fp) = step.fingerprint {
                verify_fingerprint(&path, fp)?;
            }
            return Ok(path);
        }
    }

    match fs::symlink_metadata(&config_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(BootstrapError::SymlinkRejected(config_path));
        }
        Ok(_) => return Err(BootstrapError::ConfigExists(config_path)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(BootstrapError::Io(e)),
    }

    let history_db = paths.history_db();
    let history_key = paths.fresh_history_key();
    if history_db.exists() && !history_key.exists() {
        return Err(BootstrapError::HistoryKeyMissingForExistingDb(history_db));
    }

    let config = Config::fresh_companion_template(
        paths.dedicated_identity(),
        paths.authorized_devices(),
        paths.history_db(),
        history_key,
    );

    let body = render_fresh_toml(&config)?;
    // Own temp only — never scan/delete other processes' .config.toml.*.tmp files.
    let tmp = config_dir.join(format!(".config.toml.{}.tmp", uuid::Uuid::new_v4()));
    write_create_new(&tmp, body.as_bytes(), 0o600)?;
    match publish_hard_link_no_clobber(&tmp, &config_path) {
        Ok(()) => {}
        Err(BootstrapError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(BootstrapError::ConfigExists(config_path));
        }
        Err(e) => return Err(e),
    }
    sync_dir(&config_dir)?;
    Ok(config_path)
}

fn render_fresh_toml(config: &Config) -> Result<String, BootstrapError> {
    let body = config.to_toml_string()?;
    Ok(format!(
        "# ClipSync configuration (companion fresh setup)\n\
         # Dedicated identity and ClipSync-only authorization file.\n\
         # history_key is explicit under the config directory.\n\n\
         {body}"
    ))
}

pub(super) fn file_fingerprint(path: &Path) -> Option<String> {
    ContentFingerprint::of_file(path).ok().map(|f| f.sha256_hex)
}
