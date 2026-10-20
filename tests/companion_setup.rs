//! Companion setup/resolution tests (D02).
//!
//! Uses [`BootstrapPaths::for_fixture`] only — never mutates process HOME,
//! never touches installed services, personal clipboard, or live config.

use clipsync::config::resolution::{
    resolve_config, ConfigProvenance, ExplicitKind, ResolveErrorCode, ResolveOutcome, ResolvePaths,
};
use clipsync::config::Config;
use clipsync::setup::bootstrap::{
    adopt_existing, fresh_setup, inspect_resolution, resume_setup, validate_adoption,
    BootstrapError, BootstrapMode, BootstrapPaths,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tempfile::TempDir;

fn write_minimal_config(path: &Path, listen: &str, ssh_key: &Path, auth: &Path, history_db: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(
        path,
        format!(
            r#"listen_addr = "{listen}"
advertise_name = "fixture-clipsync"
log_level = "info"

[auth]
ssh_key = "{ssh}"
authorized_keys = "{auth}"

[clipboard]
max_size = 1048576
sync_primary = true
history_size = 20
history_db = "{db}"
"#,
            listen = listen,
            ssh = ssh_key.display(),
            auth = auth.display(),
            db = history_db.display(),
        ),
    )
    .unwrap();
}

#[test]
fn fresh_eligible_empty_fixture() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let res = inspect_resolution(&paths);
    assert!(res.is_fresh_eligible());
    match res.outcome.unwrap() {
        ResolveOutcome::FreshEligible { proposed_path } => {
            assert_eq!(proposed_path, paths.config_toml());
        }
        other => panic!("unexpected {:?}", other),
    }
}

#[test]
fn missing_clipsync_config_env_hard_fails() {
    let tmp = TempDir::new().unwrap();
    let missing = tmp.path().join("absent.toml");
    let paths = ResolvePaths::for_fixture(tmp.path()).with_clipsync_config(Some(missing.clone()));
    let res = resolve_config(&paths, None);
    match res.outcome {
        Err(issue) => {
            assert!(issue.message.contains("missing") || issue.path == Some(missing));
        }
        Ok(other) => panic!("expected hard fail, got {:?}", other),
    }
}

#[test]
fn missing_explicit_flag_hard_fails() {
    let tmp = TempDir::new().unwrap();
    let paths = ResolvePaths::for_fixture(tmp.path());
    let missing = tmp.path().join("nope.toml");
    let res = resolve_config(&paths, Some((missing.clone(), ExplicitKind::Flag)));
    assert!(matches!(
        res.outcome,
        Err(ref i) if i.path.as_deref() == Some(missing.as_path())
    ));
}

#[test]
fn xdg_and_legacy_mac_conflict_when_distinct() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let resolve = paths.to_resolve_paths();
    let xdg = resolve.canonical_xdg_config_toml();
    let legacy = resolve.legacy_mac_config_toml();
    let ssh = tmp.path().join("keys/id");
    let auth = tmp.path().join("keys/auth");
    let db = tmp.path().join("hist.db");
    fs::create_dir_all(ssh.parent().unwrap()).unwrap();
    write_minimal_config(&xdg, "0.0.0.0:8484", &ssh, &auth, &db);
    write_minimal_config(&legacy, "0.0.0.0:9999", &ssh, &auth, &db);

    let res = resolve_config(&resolve, None);
    match res.outcome {
        Ok(ResolveOutcome::Conflict { candidates }) => {
            assert!(candidates.len() >= 2);
            assert!(candidates
                .iter()
                .any(|c| c.provenance == ConfigProvenance::CanonicalXdg));
            assert!(candidates
                .iter()
                .any(|c| c.provenance == ConfigProvenance::LegacyMacAppSupport));
        }
        other => panic!("expected conflict, got {:?}", other),
    }
}

#[test]
fn identical_dual_paths_select_canonical_xdg() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let resolve = paths.to_resolve_paths();
    let xdg = resolve.canonical_xdg_config_toml();
    let legacy = resolve.legacy_mac_config_toml();
    let ssh = tmp.path().join("keys/id");
    let auth = tmp.path().join("keys/auth");
    let db = tmp.path().join("hist.db");
    let body = format!(
        r#"listen_addr = "0.0.0.0:8484"
advertise_name = "same"
log_level = "info"
[auth]
ssh_key = "{ssh}"
authorized_keys = "{auth}"
[clipboard]
max_size = 1048576
sync_primary = true
history_size = 20
history_db = "{db}"
"#,
        ssh = ssh.display(),
        auth = auth.display(),
        db = db.display(),
    );
    fs::create_dir_all(xdg.parent().unwrap()).unwrap();
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&xdg, &body).unwrap();
    fs::write(&legacy, &body).unwrap();

    let res = resolve_config(&resolve, None);
    match res.outcome {
        Ok(ResolveOutcome::Selected {
            path, provenance, ..
        }) => {
            assert_eq!(path, xdg);
            assert_eq!(provenance, ConfigProvenance::CanonicalXdg);
        }
        other => panic!("expected selected canonical, got {:?}", other),
    }
}

#[test]
fn symlink_canonical_config_blocks_fresh_eligible() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let xdg = paths.config_toml();
    fs::create_dir_all(xdg.parent().unwrap()).unwrap();
    let target = tmp.path().join("elsewhere.toml");
    fs::write(&target, "listen_addr = \"0.0.0.0:8484\"\n").unwrap();
    std::os::unix::fs::symlink(&target, &xdg).unwrap();
    let res = inspect_resolution(&paths);
    match res.outcome {
        Err(ref issue) => assert_eq!(issue.code, ResolveErrorCode::ConfigUnusable),
        Ok(other) => panic!("expected ConfigUnusable, got {:?}", other),
    }
    assert!(!res.is_fresh_eligible());
}

#[test]
fn unusable_xdg_does_not_silently_select_legacy() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let resolve = paths.to_resolve_paths();
    let xdg = resolve.canonical_xdg_config_toml();
    let legacy = resolve.legacy_mac_config_toml();
    fs::create_dir_all(&xdg).unwrap();
    let ssh = tmp.path().join("keys/id");
    let auth = tmp.path().join("keys/auth");
    let db = tmp.path().join("hist.db");
    write_minimal_config(&legacy, "0.0.0.0:8484", &ssh, &auth, &db);
    let res = resolve_config(&resolve, None);
    match res.outcome {
        Err(issue) => assert_eq!(issue.code, ResolveErrorCode::ConfigUnusable),
        Ok(ResolveOutcome::Selected { .. }) => panic!("must not silently select legacy"),
        Ok(other) => panic!("expected ConfigUnusable, got {:?}", other),
    }
}

#[test]
fn fresh_setup_atomic_dedicated_layout() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let out = fresh_setup(&paths).expect("fresh_setup");
    assert_eq!(out.mode, BootstrapMode::Fresh);
    assert!(out.config_path.is_file());
    assert!(out.identity_path.is_file());
    assert!(out.auth_path.is_file());

    #[cfg(unix)]
    {
        let mode = fs::metadata(&out.identity_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = fs::metadata(paths.ssh_dir()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
    }

    assert!(!paths.home.join(".ssh/id_ed25519").exists());
    assert!(!paths.home.join(".ssh/authorized_keys").exists());

    let toml = fs::read_to_string(&out.config_path).unwrap();
    assert!(toml.contains("id_ed25519_clipsync"));
    assert!(toml.contains("authorized_devices"));
    assert!(toml.contains("history.key"));
    let cfg = Config::parse_for_resolve(&toml).unwrap();
    assert_eq!(
        cfg.clipboard.history_key.as_deref(),
        Some(paths.fresh_history_key().as_path())
    );
}

#[test]
fn fresh_setup_identity_exists_without_own_journal() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let first = fresh_setup(&paths).unwrap();
    let key_before = fs::read(&first.identity_path).unwrap();
    fs::remove_file(&first.config_path).unwrap();
    let _ = fs::remove_file(paths.journal_path());
    let err = fresh_setup(&paths).unwrap_err();
    assert!(matches!(err, BootstrapError::IdentityExists(_)));
    assert_eq!(key_before, fs::read(&first.identity_path).unwrap());
}

#[test]
fn resume_reuses_identity_only_with_journal_fingerprint() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let out = fresh_setup(&paths).unwrap();
    let key_bytes = fs::read(&out.identity_path).unwrap();
    fs::remove_file(&out.config_path).unwrap();

    let journal_path = paths.journal_path();
    let mut journal: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&journal_path).unwrap()).unwrap();
    if let Some(steps) = journal.get_mut("steps").and_then(|s| s.as_array_mut()) {
        for step in steps.iter_mut() {
            if step.get("id").and_then(|v| v.as_str()) == Some("config_write") {
                step["status"] = serde_json::json!("pending");
                step.as_object_mut().unwrap().remove("fingerprint");
                step.as_object_mut().unwrap().remove("path");
            }
        }
    }
    fs::write(
        &journal_path,
        serde_json::to_string_pretty(&journal).unwrap(),
    )
    .unwrap();

    let resumed = resume_setup(&paths).expect("resume with journal proof");
    assert!(resumed.reused_identity);
    assert_eq!(key_bytes, fs::read(&out.identity_path).unwrap());
    assert!(resumed.config_path.is_file());
}

#[test]
fn adopt_preserves_identity_auth_history_bytes() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let fresh = fresh_setup(&paths).unwrap();

    let history_db = paths.history_db();
    fs::create_dir_all(history_db.parent().unwrap()).unwrap();
    let history_bytes = b"sqlite-fixture-bytes";
    fs::write(&history_db, history_bytes).unwrap();
    // Fresh TOML sets explicit history_key under config_dir.
    let history_key = paths.fresh_history_key();
    fs::write(&history_key, [7u8; 32]).unwrap();

    let id_bytes = fs::read(&fresh.identity_path).unwrap();
    let auth_bytes = fs::read(&fresh.auth_path).unwrap();

    let report = validate_adoption(&paths, &fresh.config_path).unwrap();
    assert!(report.blocked.is_none());
    assert!(report.identity_ok);
    assert!(report.auth_ok);

    let adopted = adopt_existing(&paths, &fresh.config_path).unwrap();
    assert_eq!(adopted.mode, BootstrapMode::Adopt);
    assert_eq!(id_bytes, fs::read(&fresh.identity_path).unwrap());
    assert_eq!(auth_bytes, fs::read(&fresh.auth_path).unwrap());
    assert_eq!(history_bytes.as_slice(), fs::read(&history_db).unwrap());
}

#[test]
fn adopt_historical_history_key_when_unset() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let other = TempDir::new().unwrap();
    let gen = fresh_setup(&BootstrapPaths::for_fixture(other.path())).unwrap();
    let identity = paths.dedicated_identity();
    fs::create_dir_all(paths.ssh_dir()).unwrap();
    fs::copy(&gen.identity_path, &identity).unwrap();
    fs::copy(
        format!("{}.pub", gen.identity_path.display()),
        format!("{}.pub", identity.display()),
    )
    .unwrap();
    let auth = paths.authorized_devices();
    fs::create_dir_all(paths.config_dir()).unwrap();
    fs::write(&auth, b"").unwrap();
    let history_db = paths.history_db();
    fs::create_dir_all(history_db.parent().unwrap()).unwrap();
    fs::write(&history_db, b"db").unwrap();

    // Config omits history_key → historical platform default (macOS: App Support).
    write_minimal_config(
        &paths.config_toml(),
        "0.0.0.0:8484",
        &identity,
        &auth,
        &history_db,
    );
    let hist_key = paths.historical_default_history_key();
    #[cfg(target_os = "macos")]
    assert_ne!(
        hist_key,
        paths.fresh_history_key(),
        "macOS historical key stays under Application Support"
    );
    // Without key at historical location → blocked.
    let report = validate_adoption(&paths, &paths.config_toml()).unwrap();
    assert_eq!(
        report.blocked,
        Some(clipsync::setup::bootstrap::BootstrapErrorCode::HistoryKeyMissingForExistingDb)
    );
    fs::create_dir_all(hist_key.parent().unwrap()).unwrap();
    fs::write(&hist_key, [9u8; 32]).unwrap();
    let report2 = validate_adoption(&paths, &paths.config_toml()).unwrap();
    assert!(report2.blocked.is_none());
}

#[test]
fn adopt_blocks_general_ssh_authorized_keys() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let ssh_dir = paths.ssh_dir();
    fs::create_dir_all(&ssh_dir).unwrap();
    let general_auth = ssh_dir.join("authorized_keys");
    fs::write(&general_auth, b"ssh-ed25519 AAAA test\n").unwrap();
    let identity = ssh_dir.join("id_ed25519_clipsync");
    let other = TempDir::new().unwrap();
    let other_paths = BootstrapPaths::for_fixture(other.path());
    let gen = fresh_setup(&other_paths).unwrap();
    fs::copy(&gen.identity_path, &identity).unwrap();
    fs::copy(
        format!("{}.pub", gen.identity_path.display()),
        format!("{}.pub", identity.display()),
    )
    .unwrap();

    let config_path = paths.config_toml();
    write_minimal_config(
        &config_path,
        "0.0.0.0:8484",
        &identity,
        &general_auth,
        &paths.history_db(),
    );

    let report = validate_adoption(&paths, &config_path).unwrap();
    assert!(report.blocked.is_some());
    let err = adopt_existing(&paths, &config_path).unwrap_err();
    assert!(matches!(err, BootstrapError::AuthFileIsGeneralSsh(_)));
}

#[test]
fn interrupted_config_write_retry_succeeds_without_deleting_foreign_temps() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());

    let out = fresh_setup(&paths).unwrap();
    let key_bytes = fs::read(&out.identity_path).unwrap();
    let auth_bytes = fs::read(&out.auth_path).unwrap();

    fs::remove_file(&out.config_path).unwrap();
    let foreign_tmp = paths
        .config_dir()
        .join(".config.toml.deadbeef-interrupted.tmp");
    fs::write(&foreign_tmp, b"truncated-partial").unwrap();

    let journal_path = paths.journal_path();
    let mut journal: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&journal_path).unwrap()).unwrap();
    if let Some(steps) = journal.get_mut("steps").and_then(|s| s.as_array_mut()) {
        for step in steps.iter_mut() {
            if step.get("id").and_then(|v| v.as_str()) == Some("config_write") {
                step["status"] = serde_json::json!("pending");
                step.as_object_mut().unwrap().remove("fingerprint");
                step.as_object_mut().unwrap().remove("path");
            }
        }
    }
    fs::write(
        &journal_path,
        serde_json::to_string_pretty(&journal).unwrap(),
    )
    .unwrap();

    assert!(!out.config_path.exists());
    let resumed = resume_setup(&paths).expect("resume");
    assert!(resumed.config_path.is_file());
    assert!(
        foreign_tmp.exists(),
        "must not delete another process's temp"
    );
    let toml = fs::read_to_string(&resumed.config_path).unwrap();
    assert!(toml.contains("id_ed25519_clipsync"));
    assert!(!toml.contains("truncated-partial"));
    assert_eq!(key_bytes, fs::read(&out.identity_path).unwrap());
    assert_eq!(auth_bytes, fs::read(&out.auth_path).unwrap());
}

#[test]
fn complete_journal_missing_config_is_not_success() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let out = fresh_setup(&paths).unwrap();
    fs::remove_file(&out.config_path).unwrap();
    let err = resume_setup(&paths).unwrap_err();
    assert!(matches!(err, BootstrapError::JournalCorrupt(_)));
}

#[test]
fn history_db_without_key_blocks_adoption() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let fresh = fresh_setup(&paths).unwrap();
    let history_db = paths.history_db();
    fs::create_dir_all(history_db.parent().unwrap()).unwrap();
    fs::write(&history_db, b"db").unwrap();
    // Fresh config points at config_dir/history.key which does not exist.
    let report = validate_adoption(&paths, &fresh.config_path).unwrap();
    assert_eq!(
        report.blocked,
        Some(clipsync::setup::bootstrap::BootstrapErrorCode::HistoryKeyMissingForExistingDb)
    );
}

#[test]
fn fixture_paths_do_not_require_home_env_mutation() {
    let tmp = TempDir::new().unwrap();
    let home_before = std::env::var_os("HOME");
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let _ = fresh_setup(&paths).unwrap();
    assert_eq!(std::env::var_os("HOME"), home_before);
}

#[test]
fn symlink_config_path_rejected_on_fresh_write() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    fs::create_dir_all(paths.ssh_dir()).unwrap();
    let target = tmp.path().join("elsewhere");
    fs::write(&target, b"not-a-key").unwrap();
    std::os::unix::fs::symlink(&target, paths.dedicated_identity()).unwrap();
    let err = fresh_setup(&paths).unwrap_err();
    assert!(matches!(err, BootstrapError::SymlinkRejected(_)));
}

#[test]
fn fresh_toml_roundtrips_quoted_advertise_name() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let out = fresh_setup(&paths).unwrap();
    let mut cfg =
        Config::parse_for_resolve(&fs::read_to_string(&out.config_path).unwrap()).unwrap();
    cfg.advertise_name = r#"name"with\backslash"#.into();
    let body = cfg.to_toml_string().unwrap();
    let parsed = Config::parse_for_resolve(&body).unwrap();
    assert_eq!(parsed.advertise_name, cfg.advertise_name);
}

#[test]
fn dedicated_identity_template_for_cli_example() {
    let example = Config::generate_example();
    assert!(example.contains("id_ed25519_clipsync"));
    assert!(example.contains("authorized_devices"));
    assert!(example.contains("history.key"));
    assert!(!example.contains("ssh_key = \"~/.ssh/id_ed25519\""));
}

#[test]
fn pub_sibling_mismatch_blocks_committed_identity_resume() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    let out = fresh_setup(&paths).unwrap();
    let key_bytes = fs::read(&out.identity_path).unwrap();
    fs::remove_file(&out.config_path).unwrap();
    fs::write(
        paths.dedicated_identity_pub(),
        b"ssh-ed25519 AAAA not-ours clipsync@localhost\n",
    )
    .unwrap();

    // Inspect pending + Identity committed → resume re-enters Identity and verifies .pub.
    let journal_path = paths.journal_path();
    let mut journal: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&journal_path).unwrap()).unwrap();
    if let Some(steps) = journal.get_mut("steps").and_then(|s| s.as_array_mut()) {
        for step in steps.iter_mut() {
            let id = step.get("id").and_then(|v| v.as_str()).unwrap_or("");
            match id {
                "inspect" => {
                    step["status"] = serde_json::json!("pending");
                    step.as_object_mut().unwrap().remove("fingerprint");
                }
                "auth_file" | "config_write" => {
                    step["status"] = serde_json::json!("pending");
                    step.as_object_mut().unwrap().remove("fingerprint");
                    step.as_object_mut().unwrap().remove("path");
                }
                "identity" => {
                    step["status"] = serde_json::json!("committed");
                }
                _ => {}
            }
        }
    }
    fs::write(
        &journal_path,
        serde_json::to_string_pretty(&journal).unwrap(),
    )
    .unwrap();

    let err = resume_setup(&paths).unwrap_err();
    assert!(
        matches!(err, BootstrapError::Other(ref m) if m.contains("mismatches")),
        "expected .pub mismatch, got {:?}",
        err
    );
    assert_eq!(key_bytes, fs::read(&out.identity_path).unwrap());
}

#[cfg(target_os = "macos")]
#[test]
fn mac_historical_history_key_is_app_support_not_xdg() {
    let tmp = TempDir::new().unwrap();
    let paths = BootstrapPaths::for_fixture(tmp.path());
    assert_eq!(
        paths.historical_default_history_key(),
        paths
            .home
            .join("Library/Application Support/clipsync/history.key")
    );
    assert_eq!(
        paths.fresh_history_key(),
        paths.config_dir().join("history.key")
    );
    assert_ne!(
        paths.fresh_history_key(),
        paths.historical_default_history_key()
    );
}
