//! Dedicated Ed25519 identity publish (atomic, no-clobber, fail-closed .pub).

use crate::auth::{KeyPair, KeyType};
use crate::config::resolution::ContentFingerprint;
use std::fs;
use std::path::{Path, PathBuf};

use super::error::BootstrapError;
use super::fsutil::{
    ensure_private_dir, publish_hard_link_no_clobber, reject_symlink, sync_dir, write_create_new,
};
use super::journal::{verify_fingerprint, JournalStep, SetupJournal, SetupStepId, StepStatus};
use super::paths::BootstrapPaths;

pub(super) fn ensure_dedicated_identity(
    paths: &BootstrapPaths,
    journal: &SetupJournal,
) -> Result<(PathBuf, bool), BootstrapError> {
    let key_path = paths.dedicated_identity();
    ensure_private_dir(&paths.ssh_dir(), 0o700, &paths.home)?;

    // Own committed identity step: verify fingerprints and return (explicit reuse).
    if let Some(step) = journal.step(SetupStepId::Identity) {
        if step.status == StepStatus::Committed {
            return verify_committed_identity(paths, step);
        }
    }

    // Pre-existing dedicated key without our committed journal step → IdentityExists.
    match fs::symlink_metadata(&key_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(BootstrapError::SymlinkRejected(key_path));
        }
        Ok(_) => {
            return Err(BootstrapError::IdentityExists(key_path));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(BootstrapError::Io(e)),
    }

    // Pre-existing .pub without private is a sibling conflict.
    let pub_path = paths.dedicated_identity_pub();
    match fs::symlink_metadata(&pub_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(BootstrapError::SymlinkRejected(pub_path));
        }
        Ok(_) => {
            return Err(BootstrapError::Other(format!(
                "public key sibling exists without private key: {}",
                pub_path.display()
            )));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(BootstrapError::Io(e)),
    }

    let keypair = KeyPair::generate(KeyType::Ed25519)?;
    atomic_publish_keypair(&keypair, &key_path, &paths.home)?;
    Ok((key_path, false))
}

fn verify_committed_identity(
    paths: &BootstrapPaths,
    step: &JournalStep,
) -> Result<(PathBuf, bool), BootstrapError> {
    let key_path = step
        .path
        .clone()
        .unwrap_or_else(|| paths.dedicated_identity());
    verify_identity_readable(&key_path)?;
    if let Some(ref fp) = step.fingerprint {
        verify_fingerprint(&key_path, fp)?;
    }
    // Idempotent .pub completion for the same private key.
    ensure_matching_public_key(&key_path)?;
    Ok((key_path, true))
}

pub(super) fn verify_identity_readable(path: &Path) -> Result<(), BootstrapError> {
    reject_symlink(path)?;
    let data = fs::read(path)
        .map_err(|e| BootstrapError::IdentityUnreadable(path.to_path_buf(), e.to_string()))?;
    KeyPair::from_private_key_bytes(&data)
        .map_err(|e| BootstrapError::IdentityUnreadable(path.to_path_buf(), e.to_string()))?;
    Ok(())
}

fn ensure_matching_public_key(private_path: &Path) -> Result<(), BootstrapError> {
    let data = fs::read(private_path).map_err(|e| {
        BootstrapError::IdentityUnreadable(private_path.to_path_buf(), e.to_string())
    })?;
    let keypair = KeyPair::from_private_key_bytes(&data).map_err(|e| {
        BootstrapError::IdentityUnreadable(private_path.to_path_buf(), e.to_string())
    })?;
    let expected = format!("{} clipsync@localhost\n", keypair.public_key().to_openssh());
    let pub_path = {
        let mut p = private_path.to_path_buf();
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("id_ed25519_clipsync");
        p.set_file_name(format!("{}.pub", name));
        p
    };

    match fs::symlink_metadata(&pub_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(BootstrapError::SymlinkRejected(pub_path));
        }
        Ok(meta) if meta.file_type().is_file() => {
            let existing = fs::read_to_string(&pub_path)
                .map_err(|e| BootstrapError::IdentityUnreadable(pub_path.clone(), e.to_string()))?;
            if existing.trim() != expected.trim() {
                return Err(BootstrapError::Other(format!(
                    "public key sibling mismatches private key: {}",
                    pub_path.display()
                )));
            }
            return Ok(());
        }
        Ok(_) => {
            return Err(BootstrapError::Other(format!(
                "public key path exists but is not a regular file: {}",
                pub_path.display()
            )));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(BootstrapError::Io(e)),
    }

    // Interrupted public publish: complete idempotently for the same key.
    write_create_new(&pub_path, expected.as_bytes(), 0o644)?;
    if let Some(parent) = pub_path.parent() {
        sync_dir(parent)?;
    }
    Ok(())
}

/// Publish PKCS8 PEM + OpenSSH public key using create_new temp + hard_link (no clobber).
fn atomic_publish_keypair(
    keypair: &KeyPair,
    private_path: &Path,
    home_root: &Path,
) -> Result<(), BootstrapError> {
    let parent = private_path
        .parent()
        .ok_or_else(|| BootstrapError::Other("identity path has no parent".into()))?;
    ensure_private_dir(parent, 0o700, home_root)?;
    reject_symlink(parent)?;

    let pem = keypair_to_pkcs8_pem(keypair)?;
    let pub_content = format!("{} clipsync@localhost\n", keypair.public_key().to_openssh());
    let pub_path = {
        let mut p = private_path.to_path_buf();
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("id_ed25519_clipsync");
        p.set_file_name(format!("{}.pub", name));
        p
    };

    // Fail closed on pre-existing .pub (mismatch / foreign artifact).
    match fs::symlink_metadata(&pub_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(BootstrapError::SymlinkRejected(pub_path));
        }
        Ok(_) => {
            return Err(BootstrapError::Other(format!(
                "refusing to clobber existing public key: {}",
                pub_path.display()
            )));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(BootstrapError::Io(e)),
    }

    let tmp_priv = parent.join(format!(".id_ed25519_clipsync.{}.tmp", uuid::Uuid::new_v4()));
    let tmp_pub = parent.join(format!(
        ".id_ed25519_clipsync.pub.{}.tmp",
        uuid::Uuid::new_v4()
    ));

    write_create_new(&tmp_priv, pem.as_bytes(), 0o600)?;
    write_create_new(&tmp_pub, pub_content.as_bytes(), 0o644)?;

    match publish_hard_link_no_clobber(&tmp_priv, private_path) {
        Ok(()) => {}
        Err(BootstrapError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&tmp_pub);
            return Err(BootstrapError::IdentityExists(private_path.to_path_buf()));
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp_pub);
            return Err(e);
        }
    }

    match publish_hard_link_no_clobber(&tmp_pub, &pub_path) {
        Ok(()) => {}
        Err(BootstrapError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Private published; verify sibling rather than clobber.
            ensure_matching_public_key(private_path)?;
        }
        Err(e) => return Err(e),
    }
    sync_dir(parent)?;
    Ok(())
}

fn keypair_to_pkcs8_pem(keypair: &KeyPair) -> Result<String, BootstrapError> {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(keypair.pkcs8());
    let body = encoded
        .chars()
        .collect::<Vec<_>>()
        .chunks(64)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        body
    ))
}

pub(super) fn identity_fingerprint(path: &Path) -> Option<String> {
    ContentFingerprint::of_file(path).ok().map(|f| f.sha256_hex)
}
