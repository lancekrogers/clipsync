//! systemd --user lifecycle for the managed clipsync.service unit.

use std::fs;

use crate::service_install::{inspect_systemd_unit, render_systemd_user_unit_with_config};

use super::error::ServiceError;
use super::host::{require_success, tool_program, CommandSpec, HostCommand};
use super::paths::{
    atomic_write, require_exact_managed, restore_previous, ServicePaths, ServiceTarget, UNIT_NAME,
};
use super::{ServiceFacts, ServicePlatform};

pub fn inspect<H: HostCommand>(
    host: &H,
    paths: &ServicePaths,
) -> Result<ServiceFacts, ServiceError> {
    let unit_path = paths.systemd_user_unit();
    let mut facts = ServiceFacts {
        unit_installed: unit_path.is_file(),
        enabled_at_login: false,
        loaded: false,
        running: false,
        exec_matches: false,
        config_matches: false,
        legacy_root_present: paths.legacy_root_present(),
        binary_path: None,
        config_path: None,
        platform: ServicePlatform::SystemdUser,
        unit_path: Some(unit_path.clone()),
    };

    if facts.unit_installed {
        let content = fs::read_to_string(&unit_path)
            .map_err(|err| ServiceError::io(format!("read {}", unit_path.display()), err))?;
        let inspection = inspect_systemd_unit(&content);
        facts.binary_path = inspection.exec_binary.map(Into::into);
        facts.config_path = inspection.config_path.map(Into::into);
    }

    facts.enabled_at_login = is_enabled(host)?;
    facts.running = is_active(host)?;
    // systemd: "loaded" ≈ unit file known to manager (is-active inactive still loaded).
    facts.loaded = is_unit_loaded(host)?;
    Ok(facts)
}

pub fn install<H: HostCommand>(
    host: &H,
    paths: &ServicePaths,
    target: &ServiceTarget,
    enable_at_login: bool,
) -> Result<(), ServiceError> {
    target.require_executable()?;
    let unit_path = paths.systemd_user_unit();
    require_exact_managed(&unit_path, &paths.systemd_user_unit())?;

    let unit_exists = unit_path.is_file();
    let (prior_enabled, prior_running) = if unit_exists {
        (is_enabled(host)?, is_active(host)?)
    } else {
        (false, false)
    };
    // Restarting a running unit after repair can compete with legacy root.
    if prior_running {
        refuse_if_legacy_root(paths)?;
    }

    let binary = target.binary.to_string_lossy();
    let config = target.config.to_string_lossy();
    let content = render_systemd_user_unit_with_config(binary.as_ref(), Some(config.as_ref()))
        .map_err(ServiceError::Other)?;

    let previous = atomic_write(&unit_path, &content, 0o644)?;

    if let Err(err) = apply_install_mutators(host, enable_at_login, prior_running) {
        let mut details = Vec::new();
        if let Err(rollback_err) = restore_previous(&unit_path, previous.as_deref()) {
            details.push(rollback_err.to_string());
        }
        if let Err(e) = run_systemctl(host, &["daemon-reload"]) {
            details.push(format!("daemon-reload after restore: {e}"));
        }
        if let Err(e) = restore_enable_running(host, prior_enabled, prior_running) {
            details.push(e.to_string());
        }
        if details.is_empty() {
            return Err(err);
        }
        return Err(ServiceError::RollbackFailed {
            during: err.to_string(),
            detail: details.join("; "),
        });
    }
    Ok(())
}

fn apply_install_mutators<H: HostCommand>(
    host: &H,
    enable_at_login: bool,
    prior_running: bool,
) -> Result<(), ServiceError> {
    run_systemctl(host, &["daemon-reload"])?;
    // Truthful startup intent: enable or disable explicitly (no implied start).
    if enable_at_login {
        run_systemctl(host, &["enable", UNIT_NAME])?;
    } else {
        run_systemctl(host, &["disable", UNIT_NAME])?;
    }
    // A changed unit on disk does not update a running job — restart to replace it.
    if prior_running {
        run_systemctl(host, &["restart", UNIT_NAME])?;
    }
    Ok(())
}

fn restore_enable_running<H: HostCommand>(
    host: &H,
    prior_enabled: bool,
    prior_running: bool,
) -> Result<(), ServiceError> {
    if prior_enabled {
        run_systemctl(host, &["enable", UNIT_NAME])?;
    } else {
        run_systemctl(host, &["disable", UNIT_NAME])?;
    }
    if prior_running {
        run_systemctl(host, &["start", UNIT_NAME])?;
    } else if is_active(host)? {
        run_systemctl(host, &["stop", UNIT_NAME])?;
    }
    Ok(())
}

pub fn start<H: HostCommand>(host: &H, paths: &ServicePaths) -> Result<(), ServiceError> {
    refuse_if_legacy_root(paths)?;
    ensure_unit_installed(paths)?;
    match run_systemctl(host, &["start", UNIT_NAME]) {
        Ok(()) => Ok(()),
        Err(err) => {
            // Idempotent success when already active.
            if is_active(host)? {
                Ok(())
            } else {
                Err(err)
            }
        }
    }
}

pub(crate) fn refuse_if_legacy_root(paths: &ServicePaths) -> Result<(), ServiceError> {
    if paths.legacy_root_present() {
        return Err(ServiceError::LegacyRootConflict);
    }
    Ok(())
}

pub fn stop<H: HostCommand>(host: &H, paths: &ServicePaths) -> Result<(), ServiceError> {
    ensure_unit_installed(paths)?;
    match run_systemctl(host, &["stop", UNIT_NAME]) {
        Ok(()) => Ok(()),
        Err(err) => {
            if !is_active(host)? {
                Ok(())
            } else {
                Err(err)
            }
        }
    }
}

pub fn restart<H: HostCommand>(host: &H, paths: &ServicePaths) -> Result<(), ServiceError> {
    refuse_if_legacy_root(paths)?;
    ensure_unit_installed(paths)?;
    // Prefer atomic restart; if the manager lacks it, stop then start with stop errors propagated.
    match run_systemctl(host, &["restart", UNIT_NAME]) {
        Ok(()) => Ok(()),
        Err(_) => {
            stop(host, paths)?;
            start(host, paths)
        }
    }
}

pub fn set_startup_at_login<H: HostCommand>(
    host: &H,
    paths: &ServicePaths,
    enabled: bool,
) -> Result<(), ServiceError> {
    ensure_unit_installed(paths)?;
    if enabled {
        run_systemctl(host, &["enable", UNIT_NAME])
    } else {
        run_systemctl(host, &["disable", UNIT_NAME])
    }
}

pub fn apply_match_flags(facts: &mut ServiceFacts, target: &ServiceTarget) {
    facts.exec_matches = facts
        .binary_path
        .as_ref()
        .is_some_and(|p| p == &target.binary);
    facts.config_matches = facts
        .config_path
        .as_ref()
        .is_some_and(|p| p == &target.config);
}

/// Restore prior enable/running after install_and_start failure (bytes already restored by caller).
pub(crate) fn restore_prior_state<H: HostCommand>(
    host: &H,
    prior_enabled: bool,
    prior_running: bool,
) -> Result<(), ServiceError> {
    run_systemctl(host, &["daemon-reload"])?;
    restore_enable_running(host, prior_enabled, prior_running)
}

fn ensure_unit_installed(paths: &ServicePaths) -> Result<(), ServiceError> {
    let unit_path = paths.systemd_user_unit();
    if !unit_path.is_file() {
        return Err(ServiceError::PathInvalid {
            path: unit_path,
            reason: "managed user unit is not installed".into(),
        });
    }
    Ok(())
}

fn is_enabled<H: HostCommand>(host: &H) -> Result<bool, ServiceError> {
    let output = host.run(&systemctl_spec(&["is-enabled", UNIT_NAME]))?;
    if output.timed_out {
        return Err(ServiceError::TimedOut {
            program: "systemctl".into(),
            args: vec!["--user".into(), "is-enabled".into(), UNIT_NAME.into()],
        });
    }
    // systemctl is-enabled: 0 enabled, non-zero otherwise — not a hard failure for inspect.
    Ok(output.status == 0)
}

fn is_active<H: HostCommand>(host: &H) -> Result<bool, ServiceError> {
    let output = host.run(&systemctl_spec(&["is-active", UNIT_NAME]))?;
    if output.timed_out {
        return Err(ServiceError::TimedOut {
            program: "systemctl".into(),
            args: vec!["--user".into(), "is-active".into(), UNIT_NAME.into()],
        });
    }
    Ok(output.status == 0)
}

fn is_unit_loaded<H: HostCommand>(host: &H) -> Result<bool, ServiceError> {
    let output = host.run(&systemctl_spec(&[
        "show",
        UNIT_NAME,
        "--property=LoadState",
        "--value",
    ]))?;
    if output.timed_out {
        return Err(ServiceError::TimedOut {
            program: "systemctl".into(),
            args: vec![
                "--user".into(),
                "show".into(),
                UNIT_NAME.into(),
                "--property=LoadState".into(),
                "--value".into(),
            ],
        });
    }
    if output.status != 0 {
        return Ok(false);
    }
    Ok(output.stdout_lossy().trim() == "loaded")
}

fn run_systemctl<H: HostCommand>(host: &H, args: &[&str]) -> Result<(), ServiceError> {
    let spec = systemctl_spec(args);
    let output = host.run(&spec)?;
    require_success(&spec, output)?;
    Ok(())
}

fn systemctl_spec(args: &[&str]) -> CommandSpec {
    let mut full = vec!["--user".to_string()];
    full.extend(args.iter().map(|s| (*s).to_string()));
    CommandSpec::new(tool_program("systemctl"), full)
}
