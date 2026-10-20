//! Setup journal (paths + fingerprints only — never secrets).

use crate::config::resolution::ContentFingerprint;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use super::error::BootstrapError;
use super::fsutil::{ensure_private_dir, write_create_new};
use super::paths::BootstrapPaths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupStepId {
    Inspect,
    Identity,
    AuthFile,
    ConfigWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalStep {
    pub id: SetupStepId,
    pub status: StepStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupJournal {
    pub version: u32,
    pub mode: String,
    pub steps: Vec<JournalStep>,
}

impl SetupJournal {
    pub(super) fn fresh_template() -> Self {
        Self {
            version: 1,
            mode: "fresh".into(),
            steps: vec![
                JournalStep {
                    id: SetupStepId::Inspect,
                    status: StepStatus::Pending,
                    path: None,
                    fingerprint: None,
                },
                JournalStep {
                    id: SetupStepId::Identity,
                    status: StepStatus::Pending,
                    path: None,
                    fingerprint: None,
                },
                JournalStep {
                    id: SetupStepId::AuthFile,
                    status: StepStatus::Pending,
                    path: None,
                    fingerprint: None,
                },
                JournalStep {
                    id: SetupStepId::ConfigWrite,
                    status: StepStatus::Pending,
                    path: None,
                    fingerprint: None,
                },
            ],
        }
    }

    pub(super) fn mark(
        &mut self,
        id: SetupStepId,
        path: Option<PathBuf>,
        fingerprint: Option<String>,
    ) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.id == id) {
            step.status = StepStatus::Committed;
            step.path = path;
            step.fingerprint = fingerprint;
        }
    }

    pub(super) fn step(&self, id: SetupStepId) -> Option<&JournalStep> {
        self.steps.iter().find(|s| s.id == id)
    }

    pub(super) fn first_incomplete(&self) -> Option<SetupStepId> {
        self.steps
            .iter()
            .find(|s| s.status != StepStatus::Committed)
            .map(|s| s.id)
    }

    pub(super) fn is_complete(&self) -> bool {
        self.steps.iter().all(|s| s.status == StepStatus::Committed)
    }
}

pub(super) fn write_journal(
    paths: &BootstrapPaths,
    journal: &SetupJournal,
) -> Result<(), BootstrapError> {
    let dir = paths.setup_state_dir();
    ensure_private_dir(&dir, 0o700, &paths.home)?;
    let path = paths.journal_path();
    let body = serde_json::to_string_pretty(journal)
        .map_err(|e| BootstrapError::JournalCorrupt(e.to_string()))?;
    let tmp = dir.join(format!(".journal.{}.tmp", uuid::Uuid::new_v4()));
    write_create_new(&tmp, body.as_bytes(), 0o600)?;
    // Journal replace is intentional (same run); destination may already exist.
    fs::rename(&tmp, &path)?;
    Ok(())
}

pub(super) fn read_journal(paths: &BootstrapPaths) -> Result<SetupJournal, BootstrapError> {
    let body = fs::read_to_string(paths.journal_path())?;
    serde_json::from_str(&body).map_err(|e| BootstrapError::JournalCorrupt(e.to_string()))
}

pub(super) fn verify_fingerprint(path: &Path, expected: &str) -> Result<(), BootstrapError> {
    let fp = ContentFingerprint::of_file(path).map_err(|e| {
        BootstrapError::JournalCorrupt(format!("cannot fingerprint {}: {}", path.display(), e))
    })?;
    if fp.sha256_hex != expected {
        return Err(BootstrapError::JournalCorrupt(format!(
            "fingerprint mismatch for {}",
            path.display()
        )));
    }
    Ok(())
}

/// Verify a complete fresh journal against on-disk artifacts (no blind success).
pub(super) fn verify_complete_fresh_artifacts(
    paths: &BootstrapPaths,
    journal: &SetupJournal,
) -> Result<(), BootstrapError> {
    if journal.mode != "fresh" {
        return Err(BootstrapError::JournalCorrupt(
            "expected fresh journal".into(),
        ));
    }
    for step in &journal.steps {
        if step.status != StepStatus::Committed {
            return Err(BootstrapError::JournalCorrupt(
                "journal marked complete but has pending steps".into(),
            ));
        }
        let path = step.path.clone().ok_or_else(|| {
            BootstrapError::JournalCorrupt(format!("committed step {:?} missing path", step.id))
        })?;
        if !path.is_file() {
            return Err(BootstrapError::JournalCorrupt(format!(
                "committed artifact missing: {}",
                path.display()
            )));
        }
        if let Some(ref fp) = step.fingerprint {
            verify_fingerprint(&path, fp)?;
        }
    }
    // Cross-check canonical paths still match.
    if !paths.config_toml().is_file() {
        return Err(BootstrapError::JournalCorrupt(
            "config.toml missing after complete journal".into(),
        ));
    }
    if !paths.dedicated_identity().is_file() {
        return Err(BootstrapError::JournalCorrupt(
            "dedicated identity missing after complete journal".into(),
        ));
    }
    Ok(())
}
