//! Fixture-only desktop-user service lifecycle tests (no live systemd/launchd/HOME).

use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use clipsync::service::{
    CommandOutput, CommandSpec, HostCommand, ServiceError, ServiceOps, ServicePaths,
    ServicePlatform, ServiceTarget,
};
use clipsync::service_install::{
    inspect_launch_agent_plist, inspect_systemd_unit, render_launch_agent_plist,
    render_systemd_user_unit_with_config, systemd_quote_arg, systemd_quote_exec_path,
};
use tempfile::tempdir;

#[derive(Clone, Default)]
struct ScriptedHost {
    calls: Arc<Mutex<Vec<CommandSpec>>>,
    responses: Arc<Mutex<VecDeque<Result<CommandOutput, ServiceError>>>>,
}

impl ScriptedHost {
    fn push_ok(&self) {
        self.responses.lock().unwrap().push_back(Ok(CommandOutput {
            status: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
            timed_out: false,
        }));
    }

    fn push_status(&self, status: i32, stdout: &str) {
        self.responses.lock().unwrap().push_back(Ok(CommandOutput {
            status,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
            timed_out: false,
        }));
    }

    fn push_fail(&self, status: i32, stderr: &str) {
        self.responses.lock().unwrap().push_back(Ok(CommandOutput {
            status,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
            timed_out: false,
        }));
    }

    fn call_programs(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| format!("{} {}", c.program.display(), c.args.join(" ")))
            .collect()
    }
}

impl HostCommand for ScriptedHost {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ServiceError> {
        self.calls.lock().unwrap().push(spec.clone());
        let mut q = self.responses.lock().unwrap();
        if let Some(next) = q.pop_front() {
            next
        } else {
            Ok(CommandOutput {
                status: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
                timed_out: false,
            })
        }
    }
}

fn fixture_tree(temp: &Path) -> (ServicePaths, PathBuf, PathBuf) {
    let home = temp.join("home");
    let xdg = temp.join("xdg");
    let bin_dir = home.join(".local/bin");
    let cfg_dir = xdg.join("clipsync");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(&cfg_dir).unwrap();
    let binary = bin_dir.join("clipsync");
    fs::write(&binary, b"#!/bin/sh\nexit 0\n").unwrap();
    let mut perms = fs::metadata(&binary).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&binary, perms).unwrap();
    let config = cfg_dir.join("config.toml");
    fs::write(&config, "node_id = \"test\"\n").unwrap();
    let mut paths = ServicePaths::new(&home, &xdg, 501);
    paths.legacy_system_units = vec![temp.join("legacy/clipsync.service")];
    (paths, binary, config)
}

#[test]
fn systemd_arg_quoting_independent_of_exec_quoting() {
    assert_eq!(
        systemd_quote_exec_path("/opt/$USER/%n/bin").unwrap(),
        "\"/opt/$USER/%%n/bin\""
    );
    assert_eq!(
        systemd_quote_arg("/home/$USER/%n/config.toml").unwrap(),
        "/home/$$USER/%%n/config.toml"
    );
    let unit = render_systemd_user_unit_with_config(
        "/opt/$USER/clipsync",
        Some("/cfg/$USER/%n/config.toml"),
    )
    .unwrap();
    assert!(unit.contains("\"/opt/$USER/clipsync\""));
    assert!(unit.contains("--config /cfg/$$USER/%%n/config.toml"));
    let inspection = inspect_systemd_unit(&unit);
    assert_eq!(
        inspection.config_path.as_deref(),
        Some("/cfg/$USER/%n/config.toml")
    );
}

#[test]
fn launch_agent_xml_escape_roundtrip() {
    let plist = render_launch_agent_plist(
        "/Apps/A&B/clipsync",
        "/Users/x/<cfg>/config.toml",
        "/tmp/out",
        "/tmp/err",
        "/Users/x",
        "/Apps/A&B:/bin",
    )
    .unwrap();
    assert!(plist.contains("&amp;"));
    assert!(plist.contains("&lt;cfg&gt;"));
    let inspection = inspect_launch_agent_plist(&plist);
    assert_eq!(
        inspection.exec_binary.as_deref(),
        Some("/Apps/A&B/clipsync")
    );
    assert_eq!(
        inspection.config_path.as_deref(),
        Some("/Users/x/<cfg>/config.toml")
    );
}

#[test]
fn linux_install_start_stop_enable_separate_from_running() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let host = ScriptedHost::default();
    // install (no prior unit): daemon-reload + enable
    host.push_ok();
    host.push_ok();
    let ops = ServiceOps::with_platform(host.clone(), paths.clone(), ServicePlatform::SystemdUser);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    ops.install(&target, true).unwrap();

    let unit = fs::read_to_string(paths.systemd_user_unit()).unwrap();
    let inspection = inspect_systemd_unit(&unit);
    assert_eq!(
        inspection.exec_binary.as_deref(),
        Some(binary.to_str().unwrap())
    );
    assert_eq!(
        inspection.config_path.as_deref(),
        Some(config.to_str().unwrap())
    );

    // inspect: is-enabled=0, is-active=1, LoadState=loaded (enabled but not running)
    host.push_status(0, "enabled\n");
    host.push_status(1, "inactive\n");
    host.push_status(0, "loaded\n");
    let facts = ops.inspect_against(&target).unwrap();
    assert!(facts.unit_installed);
    assert!(facts.enabled_at_login);
    assert!(facts.loaded);
    assert!(!facts.running);
    assert!(facts.exec_matches);
    assert!(facts.config_matches);

    host.push_ok();
    ops.start().unwrap();
    host.push_ok();
    ops.stop().unwrap();
    host.push_ok();
    ops.set_startup_at_login(false).unwrap();

    let calls = host.call_programs();
    assert!(calls.iter().any(|c| c.contains("daemon-reload")));
    assert!(calls.iter().any(|c| c.contains("enable clipsync.service")));
    assert!(calls.iter().any(|c| c.contains("start clipsync.service")));
    assert!(calls.iter().any(|c| c.contains("stop clipsync.service")));
    assert!(calls.iter().any(|c| c.contains("disable clipsync.service")));
    assert!(!calls.iter().any(|c| c.contains("pkill")));
}

#[test]
fn linux_install_false_disables_without_start() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let host = ScriptedHost::default();
    host.push_ok(); // daemon-reload
    host.push_ok(); // disable
    let ops = ServiceOps::with_platform(host.clone(), paths.clone(), ServicePlatform::SystemdUser);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    ops.install(&target, false).unwrap();
    let calls = host.call_programs();
    assert!(calls.iter().any(|c| c.contains("disable clipsync.service")));
    assert!(!calls.iter().any(|c| c.contains("enable clipsync.service")));
    assert!(!calls.iter().any(|c| c.contains("start clipsync.service")));
}

#[test]
fn linux_install_rolls_back_when_enable_fails() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let unit_path = paths.systemd_user_unit();
    fs::create_dir_all(unit_path.parent().unwrap()).unwrap();
    fs::write(&unit_path, "OLD_UNIT\n").unwrap();

    let host = ScriptedHost::default();
    host.push_status(1, "disabled\n"); // prior is-enabled
    host.push_status(1, "inactive\n"); // prior is-active
    host.push_ok(); // daemon-reload
    host.push_fail(1, "enable denied");
    // rollback: restore bytes, daemon-reload, disable, (not running so no stop needed beyond check)
    host.push_ok(); // daemon-reload after restore
    host.push_ok(); // disable restore
    host.push_status(1, "inactive\n"); // is_active check in restore_enable_running
    let ops = ServiceOps::with_platform(host, paths.clone(), ServicePlatform::SystemdUser);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    let err = ops.install(&target, true).unwrap_err();
    assert!(matches!(
        err,
        ServiceError::HostCommandFailed { .. } | ServiceError::RollbackFailed { .. }
    ));
    assert_eq!(fs::read_to_string(&unit_path).unwrap(), "OLD_UNIT\n");
}

#[test]
fn linux_start_refuses_legacy_root_conflict() {
    let temp = tempdir().unwrap();
    let (mut paths, binary, config) = fixture_tree(temp.path());
    let legacy = temp.path().join("legacy/clipsync.service");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&legacy, "[Unit]\nDescription=legacy\n").unwrap();
    paths.legacy_system_units = vec![legacy];

    let host = ScriptedHost::default();
    host.push_ok(); // daemon-reload
    host.push_ok(); // enable
    let ops = ServiceOps::with_platform(host.clone(), paths.clone(), ServicePlatform::SystemdUser);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    ops.install(&target, true).unwrap();

    let err = ops.start().unwrap_err();
    assert_eq!(err, ServiceError::LegacyRootConflict);
    assert!(!host
        .call_programs()
        .iter()
        .any(|c| c.contains("start clipsync.service")));
}

#[test]
fn linux_install_and_start_refuses_legacy_before_mutation() {
    let temp = tempdir().unwrap();
    let (mut paths, binary, config) = fixture_tree(temp.path());
    let legacy = temp.path().join("legacy/clipsync.service");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&legacy, "[Unit]\nDescription=legacy\n").unwrap();
    paths.legacy_system_units = vec![legacy];
    let unit_path = paths.systemd_user_unit();

    let host = ScriptedHost::default();
    let ops = ServiceOps::with_platform(host.clone(), paths, ServicePlatform::SystemdUser);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    let err = ops.install_and_start(&target, true).unwrap_err();
    assert_eq!(err, ServiceError::LegacyRootConflict);
    assert!(!unit_path.exists());
    assert!(host.call_programs().is_empty());
}

#[test]
fn linux_restart_propagates_stop_fallback_failure() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let host = ScriptedHost::default();
    host.push_ok();
    host.push_ok();
    let ops = ServiceOps::with_platform(host.clone(), paths.clone(), ServicePlatform::SystemdUser);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    ops.install(&target, true).unwrap();

    host.push_fail(1, "restart unavailable");
    host.push_fail(1, "stop failed");
    host.push_status(0, "active\n"); // still active after failed stop
    let err = ops.restart().unwrap_err();
    assert!(matches!(err, ServiceError::HostCommandFailed { .. }));
}

#[test]
fn macos_install_enable_rolls_back_on_failure() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let plist = paths.launch_agent_plist();
    fs::create_dir_all(plist.parent().unwrap()).unwrap();
    fs::write(&plist, "OLD_PLIST\n").unwrap();

    let host = ScriptedHost::default();
    // prior inspect: not loaded, not disabled
    host.push_status(1, ""); // print
    host.push_status(0, ""); // print-disabled
    host.push_fail(1, "enable failed");
    // rollback: restore login override + service_loaded probe
    host.push_ok(); // disable (prior RunAtLoad absent → enabled false)
    host.push_status(1, ""); // not loaded after failed enable
    let ops = ServiceOps::with_platform(host, paths.clone(), ServicePlatform::Launchd);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    let err = ops.install(&target, true).unwrap_err();
    assert!(matches!(
        err,
        ServiceError::HostCommandFailed { .. } | ServiceError::RollbackFailed { .. }
    ));
    assert_eq!(fs::read_to_string(&plist).unwrap(), "OLD_PLIST\n");
}

#[test]
fn macos_install_false_disables_without_bootstrap() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let host = ScriptedHost::default();
    // no prior plist → no inspect host calls; only disable
    host.push_ok();
    let ops = ServiceOps::with_platform(host.clone(), paths.clone(), ServicePlatform::Launchd);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    ops.install(&target, false).unwrap();
    let calls = host.call_programs();
    assert!(calls.iter().any(|c| c.contains("disable")));
    assert!(!calls.iter().any(|c| c.contains("bootstrap")));
    assert!(!calls.iter().any(|c| c.contains("load ")));
    assert!(paths.launch_agent_plist().is_file());
}

#[test]
fn macos_happy_path_write_and_inspect() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let host = ScriptedHost::default();
    // install enable → enable only (no bootstrap)
    host.push_ok();
    let ops = ServiceOps::with_platform(host.clone(), paths.clone(), ServicePlatform::Launchd);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    ops.install(&target, true).unwrap();

    let content = fs::read_to_string(paths.launch_agent_plist()).unwrap();
    assert!(content.contains("--config"));
    assert!(content.contains(binary.to_str().unwrap()));

    // inspect: print running + print-disabled empty → enabled via RunAtLoad
    host.push_status(0, "state = running\n");
    host.push_status(0, "");
    let facts = ops.inspect_against(&target).unwrap();
    assert!(facts.unit_installed);
    assert!(facts.loaded);
    assert!(facts.running);
    assert!(facts.enabled_at_login);
    assert!(facts.exec_matches);
    assert!(facts.config_matches);
}

#[test]
fn macos_disabled_loaded_running_distinct() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let host = ScriptedHost::default();
    host.push_ok(); // enable on install
    let ops = ServiceOps::with_platform(host.clone(), paths.clone(), ServicePlatform::Launchd);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    ops.install(&target, true).unwrap();

    // unloaded-but-enabled: print fails, print-disabled empty, RunAtLoad true
    host.push_status(1, "Could not find service\n");
    host.push_status(0, "");
    let facts = ops.inspect().unwrap();
    assert!(facts.unit_installed);
    assert!(!facts.loaded);
    assert!(!facts.running);
    assert!(facts.enabled_at_login);

    // loaded-but-disabled: print succeeds (not running), unquoted => true
    host.push_status(0, "state = waiting\n");
    host.push_status(0, "\t\"com.clipsync\" => true\n");
    let facts = ops.inspect().unwrap();
    assert!(facts.loaded);
    assert!(!facts.running);
    assert!(!facts.enabled_at_login);
}

#[test]
fn macos_start_replaces_stale_loaded_job() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let host = ScriptedHost::default();
    host.push_ok(); // enable
    let ops = ServiceOps::with_platform(host.clone(), paths.clone(), ServicePlatform::Launchd);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    ops.install(&target, true).unwrap();

    // start: refuse legacy (none), inspect (loaded with OLD paths), enable N/A (enabled),
    // loaded_matches_disk print fails match → bootout + bootstrap
    host.push_status(0, "state = running\npath = /old/clipsync\n"); // inspect print
    host.push_status(0, ""); // inspect print-disabled
    host.push_status(0, "state = running\npath = /old/clipsync\n"); // loaded_matches_disk print
    host.push_ok(); // bootout
    host.push_ok(); // bootstrap
    ops.start().unwrap();

    let calls = host.call_programs();
    assert!(calls
        .iter()
        .any(|c| c.contains("bootout") || c.contains("unload")));
    assert!(calls
        .iter()
        .any(|c| c.contains("bootstrap") || c.contains("load")));
}

#[test]
fn macos_start_disabled_restores_startup_intent() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let host = ScriptedHost::default();
    host.push_ok(); // disable on install(false)
    let ops = ServiceOps::with_platform(host.clone(), paths.clone(), ServicePlatform::Launchd);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    ops.install(&target, false).unwrap();

    // start while disabled: inspect shows disabled, enable → bootstrap → disable
    host.push_status(1, ""); // print not loaded
    host.push_status(0, "\"com.clipsync\" => true\n"); // disabled
    host.push_ok(); // enable
    host.push_ok(); // bootstrap
    host.push_ok(); // restore disable
    ops.start().unwrap();

    let calls = host.call_programs();
    let enable_idx = calls.iter().position(|c| c.contains("enable")).unwrap();
    let bootstrap_idx = calls
        .iter()
        .position(|c| c.contains("bootstrap") || c.ends_with("load") || c.contains(" load "))
        .unwrap();
    let disable_after = calls
        .iter()
        .enumerate()
        .filter(|(i, c)| *i > bootstrap_idx && c.contains("disable"))
        .count();
    assert!(enable_idx < bootstrap_idx);
    assert!(disable_after >= 1);
}

#[test]
fn macos_restart_propagates_stop_failure() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let host = ScriptedHost::default();
    host.push_ok();
    let ops = ServiceOps::with_platform(host.clone(), paths.clone(), ServicePlatform::Launchd);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    ops.install(&target, true).unwrap();

    host.push_fail(1, "bootout failed");
    host.push_fail(1, "unload failed");
    host.push_status(0, "state = running\n"); // still loaded
    let err = ops.restart().unwrap_err();
    assert!(matches!(err, ServiceError::HostCommandFailed { .. }));
    // Must not proceed to start/bootstrap after stop failure.
    let calls = host.call_programs();
    let after_stop = calls
        .iter()
        .skip_while(|c| !c.contains("bootout") && !c.contains("unload"))
        .skip(1);
    assert!(!after_stop
        .clone()
        .any(|c| c.contains("bootstrap") || c.contains(" start")));
}

#[test]
fn macos_repair_loaded_job_on_install() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let plist = paths.launch_agent_plist();
    fs::create_dir_all(plist.parent().unwrap()).unwrap();
    // Valid-enough plist so RunAtLoad parses; content will be replaced.
    let old = render_launch_agent_plist(
        "/old/clipsync",
        config.to_str().unwrap(),
        "/tmp/o",
        "/tmp/e",
        paths.home.to_str().unwrap(),
        "/bin",
    )
    .unwrap();
    fs::write(&plist, &old).unwrap();

    let host = ScriptedHost::default();
    // prior inspect: loaded+running
    host.push_status(0, "state = running\n");
    host.push_status(0, "");
    // enable
    host.push_ok();
    // bootout + bootstrap (replace loaded job)
    host.push_ok();
    host.push_ok();
    let ops = ServiceOps::with_platform(host.clone(), paths.clone(), ServicePlatform::Launchd);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    ops.install(&target, true).unwrap();

    let content = fs::read_to_string(&plist).unwrap();
    assert!(content.contains(binary.to_str().unwrap()));
    let calls = host.call_programs();
    assert!(calls
        .iter()
        .any(|c| c.contains("bootout") || c.contains("unload")));
    assert!(calls
        .iter()
        .any(|c| c.contains("bootstrap") || c.contains("load")));
}

#[test]
fn rejects_relative_and_parent_paths() {
    let err = ServiceTarget::new("relative/clipsync", "/tmp/config.toml").unwrap_err();
    assert!(matches!(err, ServiceError::PathInvalid { .. }));
    let err = ServiceTarget::new("/tmp/../etc/clipsync", "/tmp/config.toml").unwrap_err();
    assert!(matches!(err, ServiceError::PathInvalid { .. }));
}

#[test]
fn host_command_failure_is_not_silent() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let host = ScriptedHost::default();
    host.push_ok(); // install reload
    host.push_ok(); // enable
    let ops = ServiceOps::with_platform(host.clone(), paths, ServicePlatform::SystemdUser);
    let target = ServiceTarget::new(binary, config).unwrap();
    ops.install(&target, true).unwrap();
    host.push_fail(1, "start exploded");
    host.push_status(1, "inactive\n"); // is-active after failed start
    let err = ops.start().unwrap_err();
    match err {
        ServiceError::HostCommandFailed { stderr, status, .. } => {
            assert_eq!(status, Some(1));
            assert!(stderr.contains("start exploded"));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn linux_install_and_start_restores_prior_on_start_failure() {
    let temp = tempdir().unwrap();
    let (paths, binary, config) = fixture_tree(temp.path());
    let unit_path = paths.systemd_user_unit();
    fs::create_dir_all(unit_path.parent().unwrap()).unwrap();
    fs::write(&unit_path, "OLD_UNIT\n").unwrap();

    let host = ScriptedHost::default();
    // install_and_start: inspect prior
    host.push_status(0, "enabled\n"); // is-enabled
    host.push_status(0, "active\n"); // is-active running
    host.push_status(0, "loaded\n"); // LoadState
                                     // install: prior is-enabled/is-active again
    host.push_status(0, "enabled\n");
    host.push_status(0, "active\n");
    host.push_ok(); // daemon-reload
    host.push_ok(); // enable
    host.push_ok(); // restart (was running)
                    // start fails
    host.push_fail(1, "start failed");
    host.push_status(1, "inactive\n"); // is-active after start fail
                                       // rollback: stop
    host.push_ok(); // stop
                    // restore_prior_state: daemon-reload, enable, start
    host.push_ok();
    host.push_ok();
    host.push_ok();

    let ops = ServiceOps::with_platform(host.clone(), paths.clone(), ServicePlatform::SystemdUser);
    let target = ServiceTarget::new(&binary, &config).unwrap();
    let err = ops.install_and_start(&target, true).unwrap_err();
    assert!(matches!(
        err,
        ServiceError::HostCommandFailed { .. } | ServiceError::RollbackFailed { .. }
    ));
    assert_eq!(fs::read_to_string(&unit_path).unwrap(), "OLD_UNIT\n");
}
