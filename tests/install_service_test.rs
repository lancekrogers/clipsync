//! Isolated install/service unit tests (no host systemd, clipboard, or HOME overrides).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn clipsync_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_clipsync"))
}

fn mock_systemctl() -> PathBuf {
    repo_root().join("scripts/test/mock-systemctl.sh")
}

fn harness() -> PathBuf {
    repo_root().join("scripts/test/isolated-install-harness.sh")
}

fn bash_n_check(path: &Path) {
    let output = Command::new("bash")
        .arg("-n")
        .arg(path)
        .output()
        .expect("bash -n");
    assert!(
        output.status.success(),
        "syntax check failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

struct IsolatedInstallEnv {
    xdg_config: PathBuf,
    xdg_data: PathBuf,
    install_dir: PathBuf,
    systemd_user_dir: PathBuf,
    log_file: PathBuf,
}

impl IsolatedInstallEnv {
    fn new(temp: &Path) -> Self {
        let xdg_config = temp.join("xdg-config");
        let xdg_data = temp.join("xdg-data");
        let install_dir = temp.join("prefix/bin");
        let systemd_user_dir = xdg_config.join("systemd/user");
        fs::create_dir_all(&install_dir).expect("install dir");
        fs::create_dir_all(&systemd_user_dir).expect("systemd user dir");
        Self {
            xdg_config,
            xdg_data,
            install_dir,
            systemd_user_dir,
            log_file: temp.join("systemctl.log"),
        }
    }

    fn stub_binary(&self) -> PathBuf {
        let binary = self.install_dir.join("clipsync");
        fs::write(&binary, b"#!/bin/sh\nexit 0\n").expect("binary");
        let mut perms = fs::metadata(&binary).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&binary, perms).expect("chmod");
        binary
    }

    fn run_harness(&self, case: &str, extra_env: &[(&str, &str)]) -> std::process::Output {
        let mut cmd = Command::new("bash");
        cmd.arg(self.harness_path())
            .env("CLIPSYNC_TEST_CASE", case)
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env("XDG_DATA_HOME", &self.xdg_data)
            .env("CLIPSYNC_INSTALL_DIR", &self.install_dir)
            .env("CLIPSYNC_SYSTEMD_USER_DIR", &self.systemd_user_dir)
            .env("CLIPSYNC_SYSTEMCTL", mock_systemctl())
            .env("CLIPSYNC_MOCK_SYSTEMCTL_LOG", &self.log_file)
            .env("CLIPSYNC_UNIT_RENDERER", clipsync_bin());
        for (key, value) in extra_env {
            cmd.env(key, value);
        }
        cmd.output().expect("harness")
    }

    fn harness_path(&self) -> PathBuf {
        harness()
    }

    fn unit_path(&self) -> PathBuf {
        self.systemd_user_dir.join("clipsync.service")
    }
}

#[test]
fn install_shell_scripts_parse() {
    let root = repo_root();
    for rel in [
        "scripts/install.sh",
        "scripts/install_user.sh",
        "scripts/uninstall_user.sh",
        "scripts/lib/linux_user_service.sh",
        "scripts/test/mock-systemctl.sh",
        "scripts/test/isolated-install-harness.sh",
    ] {
        bash_n_check(&root.join(rel));
    }
}

#[test]
fn linux_user_service_writes_matching_unit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let env = IsolatedInstallEnv::new(temp.path());
    let binary = env.stub_binary();

    let output = env.run_harness(
        "install_service",
        &[("CLIPSYNC_INSTALL_BIN", binary.to_str().expect("utf8"))],
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let unit = fs::read_to_string(env.unit_path()).expect("unit file");
    let inspection = clipsync::service_install::inspect_systemd_unit(&unit);
    assert_eq!(
        inspection.exec_binary.as_deref(),
        Some(binary.to_string_lossy().as_ref())
    );
    assert!(inspection.uses_foreground);
    assert!(inspection.wanted_by_graphical_session);
    assert!(inspection.orders_after_graphical_session_pre);

    let log = fs::read_to_string(&env.log_file).expect("systemctl log");
    assert!(log.contains("scope: --user"));
    assert!(log.contains("daemon-reload"));
    assert!(log.contains("enable"));
}

#[test]
fn linux_user_service_rejects_missing_binary() {
    let temp = tempfile::tempdir().expect("tempdir");
    let env = IsolatedInstallEnv::new(temp.path());
    let missing = env.install_dir.join("missing-clipsync");

    let output = env.run_harness(
        "missing_binary",
        &[("CLIPSYNC_INSTALL_BIN", missing.to_str().expect("utf8"))],
    );
    assert!(output.status.success());
}

#[test]
fn linux_user_service_reload_failure_propagates() {
    let temp = tempfile::tempdir().expect("tempdir");
    let env = IsolatedInstallEnv::new(temp.path());
    let binary = env.stub_binary();

    let output = env.run_harness(
        "reload_fail",
        &[
            ("CLIPSYNC_INSTALL_BIN", binary.to_str().expect("utf8")),
            ("CLIPSYNC_MOCK_SYSTEMCTL_RELOAD_FAIL", "1"),
        ],
    );
    assert!(output.status.success());
}

#[test]
fn linux_user_service_enable_failure_propagates() {
    let temp = tempfile::tempdir().expect("tempdir");
    let env = IsolatedInstallEnv::new(temp.path());
    let binary = env.stub_binary();

    let output = env.run_harness(
        "enable_fail",
        &[
            ("CLIPSYNC_INSTALL_BIN", binary.to_str().expect("utf8")),
            ("CLIPSYNC_MOCK_SYSTEMCTL_ENABLE_FAIL", "1"),
        ],
    );
    assert!(output.status.success());
}

#[test]
fn linux_user_service_quotes_paths_with_spaces_and_special_chars() {
    let temp = tempfile::tempdir().expect("tempdir");
    let spaced_install = temp.path().join("prefix with spaces/bin");
    fs::create_dir_all(&spaced_install).expect("spaced install dir");
    let xdg_config = temp.path().join("xdg cfg");
    let systemd_user_dir = xdg_config.join("systemd/user");
    fs::create_dir_all(&systemd_user_dir).expect("unit dir");

    let binary = spaced_install.join("clipsync");
    fs::write(&binary, b"#!/bin/sh\nexit 0\n").expect("binary");
    let mut perms = fs::metadata(&binary).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&binary, perms).expect("chmod");

    let special = temp.path().join("apps").join("a&b$%");
    fs::create_dir_all(&special).expect("special dir");
    let special_bin = special.join("clipsync");
    fs::copy(clipsync_bin(), &special_bin).expect("copy real binary");
    let mut perms = fs::metadata(&special_bin).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&special_bin, perms).expect("chmod");

    let unit_out = Command::new(&special_bin)
        .arg("print-user-unit")
        .arg("--binary")
        .arg(&special_bin)
        .output()
        .expect("print-user-unit");
    assert!(unit_out.status.success());
    let unit = String::from_utf8_lossy(&unit_out.stdout);
    assert!(unit.contains("graphical-session-pre.target"));
    assert!(unit.contains("start --foreground"));

    let output = Command::new("bash")
        .arg(harness())
        .env("CLIPSYNC_TEST_CASE", "write_unit")
        .env("XDG_CONFIG_HOME", &xdg_config)
        .env("CLIPSYNC_SYSTEMD_USER_DIR", &systemd_user_dir)
        .env("CLIPSYNC_UNIT_RENDERER", clipsync_bin())
        .env("CLIPSYNC_INSTALL_BIN", &binary)
        .output()
        .expect("write unit");
    assert!(output.status.success());

    let unit = fs::read_to_string(systemd_user_dir.join("clipsync.service")).expect("unit");
    assert!(unit.contains("prefix with spaces"));
}

#[test]
fn rust_render_matches_inspection() {
    let unit =
        clipsync::service_install::render_systemd_user_unit("/opt/bin/clipsync").expect("render");
    let inspection = clipsync::service_install::inspect_systemd_unit(&unit);
    assert_eq!(inspection.exec_binary.as_deref(), Some("/opt/bin/clipsync"));
    assert!(inspection.uses_foreground);
}

#[test]
fn install_start_failure_propagates() {
    let temp = tempfile::tempdir().expect("tempdir");
    let env = IsolatedInstallEnv::new(temp.path());
    let binary = env.stub_binary();

    let output = env.run_harness(
        "start_fail",
        &[
            ("CLIPSYNC_INSTALL_BIN", binary.to_str().expect("utf8")),
            ("CLIPSYNC_MOCK_SYSTEMCTL_START_FAIL", "1"),
        ],
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn config_init_fresh_and_reinstall_preserves_existing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let env = IsolatedInstallEnv::new(temp.path());
    let binary = clipsync_bin();

    let first = Command::new("bash")
        .arg(harness())
        .env("CLIPSYNC_TEST_CASE", "ensure_config_fresh")
        .env("XDG_CONFIG_HOME", &env.xdg_config)
        .env("CLIPSYNC_INSTALL_BIN", &binary)
        .output()
        .expect("config fresh");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let config_path = env.xdg_config.join("clipsync/config.toml");
    let initial = fs::read_to_string(&config_path).expect("config");
    assert!(!initial.is_empty());
    fs::write(&config_path, "# preserved marker\n").expect("marker");

    let second = Command::new("bash")
        .arg(harness())
        .env("CLIPSYNC_TEST_CASE", "ensure_config_reinstall")
        .env("XDG_CONFIG_HOME", &env.xdg_config)
        .env("CLIPSYNC_INSTALL_BIN", &binary)
        .output()
        .expect("config reinstall");
    assert!(second.status.success());

    let after = fs::read_to_string(&config_path).expect("config after");
    assert!(after.contains("preserved marker"));
}

#[test]
fn print_user_unit_rejects_unsupported_path() {
    let output = Command::new(clipsync_bin())
        .arg("print-user-unit")
        .arg("--binary")
        .arg("bad\npath")
        .output()
        .expect("cli");
    assert!(!output.status.success());
}
