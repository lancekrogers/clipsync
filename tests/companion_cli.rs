//! CLI integration regressions for companion foundation (D02).
//!
//! Temp keys + explicit `--config` only. Never runs `clipsync start|stop|restart`
//! against the host. Never mutates personal clipboard or installed services.

use clipsync::auth::{AuthConfig, Authenticator, KeyPair, KeyType, SshAuthenticator};
use clipsync::config::Config;
use clipsync::service_install::inspect_systemd_unit;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use tempfile::TempDir;

fn clipsync_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_clipsync"))
}

struct Fixture {
    _dir: TempDir,
    config_path: PathBuf,
    auth_path: PathBuf,
    identity: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let cfg_dir = dir.path().join("cfg");
        let data_dir = dir.path().join("data");
        fs::create_dir_all(&cfg_dir).unwrap();
        fs::create_dir_all(&data_dir).unwrap();

        let identity = cfg_dir.join("id_ed25519_clipsync");
        let key = KeyPair::generate(KeyType::Ed25519).unwrap();
        key.save_to_file(&identity).await.unwrap();

        let auth_path = cfg_dir.join("authorized_devices");
        fs::write(&auth_path, "").unwrap();
        let history_db = data_dir.join("history.db");
        let history_key = cfg_dir.join("history.key");
        fs::write(&history_key, [0u8; 32]).unwrap();

        let config = Config::fresh_companion_template(
            identity.clone(),
            auth_path.clone(),
            history_db,
            history_key,
        );
        let config_path = cfg_dir.join("config.toml");
        fs::write(&config_path, config.to_toml_string().unwrap()).unwrap();

        Self {
            _dir: dir,
            config_path,
            auth_path,
            identity,
        }
    }

    fn cli(&self) -> Command {
        let mut cmd = Command::new(clipsync_bin());
        cmd.arg("--config").arg(&self.config_path);
        // Keep the subprocess from adopting the developer's CLIPSYNC_CONFIG.
        cmd.env_remove("CLIPSYNC_CONFIG");
        cmd
    }
}

fn peer_openssh() -> (KeyPair, String) {
    let key = KeyPair::generate(KeyType::Ed25519).unwrap();
    let openssh = key.public_key().to_openssh();
    (key, openssh)
}

#[tokio::test]
async fn auth_add_and_remove_via_real_cli() {
    let fx = Fixture::new().await;
    let (peer, openssh) = peer_openssh();
    let fingerprint = peer.public_key().fingerprint();

    let add = fx
        .cli()
        .args(["auth", "add"])
        .arg(&openssh)
        .arg("--name")
        .arg("cli-peer")
        .output()
        .expect("auth add");
    assert!(
        add.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&add.stdout),
        String::from_utf8_lossy(&add.stderr)
    );
    let stored = fs::read_to_string(&fx.auth_path).unwrap();
    assert!(
        stored.contains(&openssh) || stored.contains(fingerprint.trim_start_matches("SHA256:"))
    );
    assert!(stored.contains("cli-peer"));

    let list = fx.cli().args(["auth", "list"]).output().unwrap();
    assert!(list.status.success());
    let list_out = String::from_utf8_lossy(&list.stdout);
    assert!(list_out.contains(&fingerprint));

    let remove = fx
        .cli()
        .args(["auth", "remove"])
        .arg(&fingerprint)
        .output()
        .unwrap();
    assert!(
        remove.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&remove.stderr)
    );
    let after = fs::read_to_string(&fx.auth_path).unwrap();
    assert!(!after.contains(&openssh));
    assert!(!after.contains("cli-peer"));
}

#[tokio::test]
async fn concurrent_independent_cli_adds_retain_both_pins() {
    let fx = Fixture::new().await;
    let (a, openssh_a) = peer_openssh();
    let (b, openssh_b) = peer_openssh();
    let fp_a = a.public_key().fingerprint();
    let fp_b = b.public_key().fingerprint();

    let mut child_a = fx
        .cli()
        .args(["auth", "add"])
        .arg(&openssh_a)
        .arg("--name")
        .arg("peer-a")
        .spawn()
        .unwrap();
    let mut child_b = fx
        .cli()
        .args(["auth", "add"])
        .arg(&openssh_b)
        .arg("--name")
        .arg("peer-b")
        .spawn()
        .unwrap();
    let status_a = child_a.wait().unwrap();
    let status_b = child_b.wait().unwrap();
    assert!(status_a.success(), "peer-a add failed");
    assert!(status_b.success(), "peer-b add failed");

    let body = fs::read_to_string(&fx.auth_path).unwrap();
    assert!(
        body.contains("peer-a") && body.contains("peer-b"),
        "both pins must survive concurrent CLI adds:\n{body}"
    );
    assert!(body.contains(&openssh_a) || body.contains(fp_a.trim_start_matches("SHA256:")));
    assert!(body.contains(&openssh_b) || body.contains(fp_b.trim_start_matches("SHA256:")));
}

#[tokio::test]
async fn running_ssh_authenticator_observes_cli_revoke_and_add() {
    let fx = Fixture::new().await;
    let auth = Arc::new(
        SshAuthenticator::new(AuthConfig {
            private_key_path: fx.identity.clone(),
            authorized_keys_path: fx.auth_path.clone(),
            generate_if_missing: false,
        })
        .await
        .unwrap(),
    );

    let (peer, openssh) = peer_openssh();
    let fingerprint = peer.public_key().fingerprint();
    assert!(!auth.is_authorized(&peer.public_key()).await.unwrap());

    let add = fx
        .cli()
        .args(["auth", "add"])
        .arg(&openssh)
        .arg("--name")
        .arg("observed")
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    assert!(
        auth.is_authorized(&peer.public_key()).await.unwrap(),
        "running SshAuthenticator must observe CLI auth add"
    );

    let remove = fx
        .cli()
        .args(["auth", "remove"])
        .arg(&fingerprint)
        .output()
        .unwrap();
    assert!(remove.status.success());
    assert!(
        !auth.is_authorized(&peer.public_key()).await.unwrap(),
        "running SshAuthenticator must observe CLI auth remove"
    );
}

#[tokio::test]
async fn missing_explicit_config_fails_without_artifacts() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("no-such-config.toml");
    let sentinel_auth = dir.path().join("must-not-create-auth");
    let before: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();

    let output = Command::new(clipsync_bin())
        .arg("--config")
        .arg(&missing)
        .args(["auth", "add"])
        .arg("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJustAPlaceholderKeyMaterialXXXX not-real")
        .env_remove("CLIPSYNC_CONFIG")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "missing --config must hard-fail: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        err.to_lowercase().contains("missing")
            || err.to_lowercase().contains("no such")
            || err.contains("config")
            || err.contains(&missing.display().to_string()),
        "expected missing-config error, got: {err}"
    );
    assert!(!sentinel_auth.exists());
    let after: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        before, after,
        "missing explicit config must leave no new artifacts under the fixture root"
    );
}

#[tokio::test]
async fn invalid_explicit_config_fails_without_auth_artifacts() {
    let dir = TempDir::new().unwrap();
    let bad = dir.path().join("bad.toml");
    fs::write(&bad, "this is not = valid toml [[[").unwrap();
    let auth_guess = dir.path().join("authorized_devices");

    let output = Command::new(clipsync_bin())
        .arg("--config")
        .arg(&bad)
        .args(["auth", "list"])
        .env_remove("CLIPSYNC_CONFIG")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!auth_guess.exists());
}

#[test]
fn print_user_unit_embeds_explicit_config_with_quoting() {
    let dir = TempDir::new().unwrap();
    let bin_dir = dir.path().join("prefix with spaces");
    let cfg_dir = dir.path().join("cfg $USER %n");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(&cfg_dir).unwrap();

    let binary = bin_dir.join("clipsync");
    fs::write(&binary, b"#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&binary).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&binary, perms).unwrap();
    }
    let config = cfg_dir.join("config.toml");
    fs::write(&config, "# fixture\n").unwrap();

    let output = Command::new(clipsync_bin())
        .arg("print-user-unit")
        .arg("--binary")
        .arg(&binary)
        .arg("--config")
        .arg(&config)
        .env_remove("CLIPSYNC_CONFIG")
        .output()
        .expect("print-user-unit");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let unit = String::from_utf8_lossy(&output.stdout);
    assert!(unit.contains("start --foreground"));
    assert!(unit.contains("--config"));
    assert!(
        unit.contains("$$USER") || unit.contains("$USER"),
        "config $ must be systemd-escaped: {unit}"
    );
    assert!(
        unit.contains("%%n") || unit.contains("%n"),
        "config % must be systemd-escaped: {unit}"
    );
    let inspection = inspect_systemd_unit(&unit);
    assert_eq!(
        inspection.exec_binary.as_deref(),
        Some(binary.to_str().unwrap())
    );
    assert_eq!(
        inspection.config_path.as_deref(),
        Some(config.to_str().unwrap()),
        "quoted ExecStart must round-trip the explicit config path"
    );
}

#[test]
fn print_user_unit_rejects_relative_binary() {
    let output = Command::new(clipsync_bin())
        .arg("print-user-unit")
        .arg("--binary")
        .arg("relative/clipsync")
        .env_remove("CLIPSYNC_CONFIG")
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn fresh_cli_refuses_implicit_runtime_and_auth_writes() {
    let dir = TempDir::new().unwrap();
    let (_, key) = peer_openssh();
    let output = Command::new(clipsync_bin())
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env_remove("CLIPSYNC_CONFIG")
        .args(["auth", "add", &key])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not configured"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn print_user_unit_accepts_global_config_without_loading_it() {
    let output = Command::new(clipsync_bin())
        .args([
            "--config",
            "/tmp/fixture config.toml",
            "print-user-unit",
            "--binary",
            "/tmp/clipsync",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let unit = String::from_utf8(output.stdout).unwrap();
    let parsed = inspect_systemd_unit(&unit);
    assert_eq!(
        parsed.config_path,
        Some("/tmp/fixture config.toml".to_owned())
    );
}

#[test]
fn version_is_available_before_setup() {
    let dir = TempDir::new().unwrap();
    let output = Command::new(clipsync_bin())
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env_remove("CLIPSYNC_CONFIG")
        .arg("version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Build Information"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn cli_name_cannot_inject_another_authorized_key() {
    let fx = Fixture::new().await;
    let (_, key) = peer_openssh();
    let (_, injected) = peer_openssh();
    let output = fx
        .cli()
        .args(["auth", "add", &key, "--name", &format!("label\n{injected}")])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(fs::read_to_string(fx.auth_path).unwrap().is_empty());
}
