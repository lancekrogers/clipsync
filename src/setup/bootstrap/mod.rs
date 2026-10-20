//! Companion offline bootstrap: fresh setup, adoption validation, journaled retry (D02).
//!
//! Uses explicit [`BootstrapPaths`] so tests never mutate process `HOME`.
//! Atomic identity/config publish via create_new temp + hard_link; no shell `ssh-keygen`.
//! Journal holds paths/fingerprints only (never secrets).

mod adopt;
mod error;
mod fsutil;
mod identity;
mod journal;
mod paths;
mod persist;

pub use adopt::{
    adopt_existing, validate_adoption, AdoptionReport, BootstrapMode, BootstrapOutcome,
};
pub use error::{BootstrapError, BootstrapErrorCode};
pub use journal::{JournalStep, SetupJournal, SetupStepId, StepStatus};
pub use paths::BootstrapPaths;

use crate::config::resolution::{resolve_config, ExplicitKind, ResolveErrorCode, ResolveOutcome};
use fsutil::SetupLock;
use identity::{ensure_dedicated_identity, identity_fingerprint};
use journal::{
    read_journal, verify_complete_fresh_artifacts, write_journal, SetupJournal as Journal,
};
use persist::{ensure_auth_file, file_fingerprint, write_fresh_config_atomic};

/// Inspect installation without side effects (config candidates only).
pub fn inspect_resolution(paths: &BootstrapPaths) -> crate::config::resolution::ConfigResolution {
    resolve_config(&paths.to_resolve_paths(), None)
}

/// Resolve with optional explicit path (CLI `--config` / API).
pub fn resolve_with_explicit(
    paths: &BootstrapPaths,
    explicit: Option<(std::path::PathBuf, ExplicitKind)>,
) -> crate::config::resolution::ConfigResolution {
    resolve_config(&paths.to_resolve_paths(), explicit)
}

/// Create canonical fresh layout (dedicated key + auth + config) with recoverable journal.
pub fn fresh_setup(paths: &BootstrapPaths) -> Result<BootstrapOutcome, BootstrapError> {
    let _lock = SetupLock::try_acquire(
        &paths.setup_lock_path(),
        &paths.setup_state_dir(),
        &paths.home,
    )?;

    let resolution = resolve_config(&paths.to_resolve_paths(), None);
    match resolution.outcome {
        Ok(ResolveOutcome::FreshEligible { .. }) => {}
        Ok(ResolveOutcome::Conflict { .. }) => return Err(BootstrapError::ConfigConflict),
        Ok(ResolveOutcome::Selected { path, .. }) => {
            return Err(BootstrapError::NotFreshEligible(format!(
                "config already present at {}",
                path.display()
            )));
        }
        Err(issue) if issue.code == ResolveErrorCode::ConfigConflict => {
            return Err(BootstrapError::ConfigConflict);
        }
        Err(issue) => {
            return Err(BootstrapError::Other(issue.message));
        }
    }

    // Resume if a fresh journal already exists.
    if paths.journal_path().exists() {
        if let Ok(existing) = read_journal(paths) {
            if existing.mode == "fresh" {
                if existing.is_complete() {
                    verify_complete_fresh_artifacts(paths, &existing)?;
                    return Ok(BootstrapOutcome {
                        config_path: paths.config_toml(),
                        identity_path: paths.dedicated_identity(),
                        auth_path: paths.authorized_devices(),
                        journal_path: paths.journal_path(),
                        reused_identity: true,
                        mode: BootstrapMode::Fresh,
                    });
                }
                let start = existing
                    .first_incomplete()
                    .ok_or_else(|| BootstrapError::JournalCorrupt("no incomplete step".into()))?;
                let mut journal = existing;
                return run_fresh_from_step(paths, &mut journal, start);
            }
        }
    }

    let mut journal = Journal::fresh_template();
    write_journal(paths, &journal)?;
    run_fresh_from_step(paths, &mut journal, SetupStepId::Inspect)
}

/// Resume interrupted journaled setup from the first incomplete step.
pub fn resume_setup(paths: &BootstrapPaths) -> Result<BootstrapOutcome, BootstrapError> {
    let _lock = SetupLock::try_acquire(
        &paths.setup_lock_path(),
        &paths.setup_state_dir(),
        &paths.home,
    )?;

    let mut journal = read_journal(paths)?;
    if journal.mode != "fresh" {
        return Err(BootstrapError::JournalCorrupt(
            "resume_setup only supports fresh journals".into(),
        ));
    }
    if journal.is_complete() {
        verify_complete_fresh_artifacts(paths, &journal)?;
        return Ok(BootstrapOutcome {
            config_path: paths.config_toml(),
            identity_path: paths.dedicated_identity(),
            auth_path: paths.authorized_devices(),
            journal_path: paths.journal_path(),
            reused_identity: true,
            mode: BootstrapMode::Fresh,
        });
    }
    let start = journal
        .first_incomplete()
        .ok_or_else(|| BootstrapError::JournalCorrupt("no incomplete step".into()))?;
    run_fresh_from_step(paths, &mut journal, start)
}

fn run_fresh_from_step(
    paths: &BootstrapPaths,
    journal: &mut Journal,
    start: SetupStepId,
) -> Result<BootstrapOutcome, BootstrapError> {
    let order = [
        SetupStepId::Inspect,
        SetupStepId::Identity,
        SetupStepId::AuthFile,
        SetupStepId::ConfigWrite,
    ];
    let start_idx = order
        .iter()
        .position(|s| *s == start)
        .ok_or_else(|| BootstrapError::JournalCorrupt("unknown step".into()))?;

    // If identity was already committed before this resume window, we reused it.
    let mut reused_identity = journal
        .step(SetupStepId::Identity)
        .is_some_and(|s| s.status == StepStatus::Committed);

    for step in &order[start_idx..] {
        match step {
            SetupStepId::Inspect => {
                journal.mark(SetupStepId::Inspect, Some(paths.config_toml()), None);
                write_journal(paths, journal)?;
            }
            SetupStepId::Identity => {
                let (path, reused) = ensure_dedicated_identity(paths, journal)?;
                reused_identity = reused;
                let fp = identity_fingerprint(&path);
                journal.mark(SetupStepId::Identity, Some(path), fp);
                write_journal(paths, journal)?;
            }
            SetupStepId::AuthFile => {
                let auth = ensure_auth_file(paths, journal)?;
                let fp = file_fingerprint(&auth);
                journal.mark(SetupStepId::AuthFile, Some(auth), fp);
                write_journal(paths, journal)?;
            }
            SetupStepId::ConfigWrite => {
                let config_path = write_fresh_config_atomic(paths, journal)?;
                let fp = file_fingerprint(&config_path);
                journal.mark(SetupStepId::ConfigWrite, Some(config_path), fp);
                write_journal(paths, journal)?;
            }
        }
    }

    verify_complete_fresh_artifacts(paths, journal)?;

    Ok(BootstrapOutcome {
        config_path: paths.config_toml(),
        identity_path: paths.dedicated_identity(),
        auth_path: paths.authorized_devices(),
        journal_path: paths.journal_path(),
        reused_identity,
        mode: BootstrapMode::Fresh,
    })
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use crate::config::Config;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn fresh_setup_creates_dedicated_artifacts() {
        let tmp = TempDir::new().unwrap();
        let paths = BootstrapPaths::for_fixture(tmp.path());
        let out = fresh_setup(&paths).expect("fresh");
        assert!(out.config_path.is_file());
        assert!(out.identity_path.is_file());
        assert!(out.auth_path.is_file());
        assert!(!paths.home.join(".ssh/id_ed25519").exists());
        assert!(!paths.home.join(".ssh/authorized_keys").exists());
        let toml = fs::read_to_string(&out.config_path).unwrap();
        assert!(toml.contains("id_ed25519_clipsync"));
        assert!(toml.contains("authorized_devices"));
        assert!(toml.contains("history.key"));
    }

    #[test]
    fn fresh_fails_identity_exists_without_journal() {
        let tmp = TempDir::new().unwrap();
        let paths = BootstrapPaths::for_fixture(tmp.path());
        let first = fresh_setup(&paths).unwrap();
        fs::remove_file(&first.config_path).unwrap();
        let _ = fs::remove_file(paths.journal_path());
        let err = fresh_setup(&paths).unwrap_err();
        assert!(matches!(err, BootstrapError::IdentityExists(_)));
    }

    #[test]
    fn fresh_history_key_is_explicit_under_config_dir() {
        let tmp = TempDir::new().unwrap();
        let paths = BootstrapPaths::for_fixture(tmp.path());
        let out = fresh_setup(&paths).unwrap();
        let cfg =
            Config::parse_for_resolve(&fs::read_to_string(&out.config_path).unwrap()).unwrap();
        assert_eq!(
            cfg.clipboard.history_key.as_deref(),
            Some(paths.fresh_history_key().as_path())
        );
    }

    #[test]
    fn complete_journal_without_artifacts_fails() {
        let tmp = TempDir::new().unwrap();
        let paths = BootstrapPaths::for_fixture(tmp.path());
        let out = fresh_setup(&paths).unwrap();
        fs::remove_file(&out.config_path).unwrap();
        let err = resume_setup(&paths).unwrap_err();
        assert!(matches!(err, BootstrapError::JournalCorrupt(_)));
    }
}
