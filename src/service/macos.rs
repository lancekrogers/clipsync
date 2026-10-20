//! launchd LaunchAgent lifecycle for com.clipsync.

use std::fs;
use std::path::{Path, PathBuf};

use crate::service_install::{inspect_launch_agent_plist, render_launch_agent_plist};

use super::error::ServiceError;
use super::host::{require_success, tool_program, CommandSpec, HostCommand};
use super::paths::{
    atomic_write, ensure_managed_dir, restore_previous, ServicePaths, ServiceTarget,
    LAUNCH_AGENT_LABEL,
};
use super::{ServiceFacts, ServicePlatform};

pub fn inspect<H: HostCommand>(
    host: &H,
    paths: &ServicePaths,
) -> Result<ServiceFacts, ServiceError> {
    let plist_path = paths.launch_agent_plist();
    let mut facts = ServiceFacts {
        unit_installed: plist_path.is_file(),
        enabled_at_login: false,
        loaded: false,
        running: false,
        exec_matches: false,
        config_matches: false,
        legacy_root_present: paths.legacy_root_present(),
        binary_path: None,
        config_path: None,
        platform: ServicePlatform::Launchd,
        unit_path: Some(plist_path.clone()),
    };

    let mut run_at_load = false;
    if facts.unit_installed {
        let content = fs::read_to_string(&plist_path)
            .map_err(|err| ServiceError::io(format!("read {}", plist_path.display()), err))?;
        let inspection = inspect_launch_agent_plist(&content);
        if inspection.label.as_deref() != Some(LAUNCH_AGENT_LABEL) && inspection.label.is_some() {
            return Err(ServiceError::UnrecognizedUnit);
        }
        facts.binary_path = inspection.exec_binary.map(Into::into);
        facts.config_path = inspection.config_path.map(Into::into);
        run_at_load = inspection.run_at_load;
        // Base login intent from plist presence + RunAtLoad (not loaded/running).
        facts.enabled_at_login = run_at_load;
    }

    let print = host.run(&launchctl_spec(&[
        "print".into(),
        paths.launchd_service_target(),
    ]))?;
    if print.timed_out {
        return Err(ServiceError::TimedOut {
            program: "launchctl".into(),
            args: vec!["print".into(), paths.launchd_service_target()],
        });
    }
    if print.status == 0 {
        let body = print.stdout_lossy();
        facts.loaded = true;
        facts.running = body.contains("state = running");
    }

    // Disabled override from print-disabled (unquoted `=> true` or quoted).
    if is_disabled_in_domain(host, paths)? {
        facts.enabled_at_login = false;
    } else if facts.unit_installed && !run_at_load {
        facts.enabled_at_login = false;
    }

    Ok(facts)
}

pub fn install<H: HostCommand>(
    host: &H,
    paths: &ServicePaths,
    target: &ServiceTarget,
    enable_at_login: bool,
) -> Result<(), ServiceError> {
    target.require_executable()?;

    let plist_path = paths.launch_agent_plist();
    // Snapshot prior manager state before any mutation (no print when plist absent).
    let prior = if plist_path.is_file() {
        inspect(host, paths)?
    } else {
        ServiceFacts {
            unit_installed: false,
            enabled_at_login: false,
            loaded: false,
            running: false,
            exec_matches: false,
            config_matches: false,
            legacy_root_present: paths.legacy_root_present(),
            binary_path: None,
            config_path: None,
            platform: ServicePlatform::Launchd,
            unit_path: Some(plist_path.clone()),
        }
    };
    // Reloading a loaded job (or bootstrapping after repair) can start KeepAlive — refuse first.
    if prior.loaded || prior.running {
        refuse_if_legacy_root(paths)?;
    }

    ensure_managed_dir(
        plist_path
            .parent()
            .ok_or_else(|| ServiceError::PathInvalid {
                path: plist_path.clone(),
                reason: "plist has no parent".into(),
            })?,
        0o755,
    )?;

    let config_parent = target
        .config
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.home.clone());
    let stdout_path = config_parent.join("clipsync.out");
    let stderr_path = config_parent.join("clipsync.err");
    let path_env = format!(
        "{}:/usr/local/bin:/usr/bin:/bin",
        target
            .binary
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "/usr/local/bin".into())
    );

    let content = render_launch_agent_plist(
        &target.binary.to_string_lossy(),
        &target.config.to_string_lossy(),
        &stdout_path.to_string_lossy(),
        &stderr_path.to_string_lossy(),
        &paths.home.to_string_lossy(),
        &path_env,
    )
    .map_err(ServiceError::Other)?;

    let previous = atomic_write(&plist_path, &content, 0o644)?;

    if let Err(err) = apply_install_mutators(host, paths, &plist_path, enable_at_login, &prior) {
        let mut details = Vec::new();
        if let Err(rollback_err) = restore_previous(&plist_path, previous.as_deref()) {
            details.push(rollback_err.to_string());
        }
        if let Err(e) = restore_login_and_runtime(host, paths, &plist_path, &prior) {
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
    paths: &ServicePaths,
    plist_path: &Path,
    enable_at_login: bool,
    prior: &ServiceFacts,
) -> Result<(), ServiceError> {
    // Explicit enable/disable without implying bootstrap/start.
    apply_startup_override(host, paths, enable_at_login)?;

    // A changed disk plist alone does not update a loaded job — replace it.
    if prior.loaded {
        bootout(host, paths, plist_path)?;
        if prior.running {
            // Restore prior running state with the new job definition.
            bootstrap(host, paths, plist_path)?;
            // If install asked for disabled-at-login, restore that intent without stopping.
            if !enable_at_login {
                apply_startup_override(host, paths, false)?;
            }
        }
    }
    Ok(())
}

pub fn start<H: HostCommand>(host: &H, paths: &ServicePaths) -> Result<(), ServiceError> {
    // Legacy conflict before ANY start mutation.
    refuse_if_legacy_root(paths)?;
    let plist_path = ensure_plist_installed(paths)?;

    let prior = inspect(host, paths)?;
    let was_disabled = !prior.enabled_at_login;

    // Start a disabled service via explicit enable → bootstrap, then restore disable intent.
    if was_disabled {
        apply_startup_override(host, paths, true)?;
    }

    let result = (|| -> Result<(), ServiceError> {
        if prior.loaded {
            if loaded_matches_disk(host, paths, &plist_path)? {
                // Already loaded with expected ProgramArguments — success (not unavailable).
                return Ok(());
            }
            // Stale loaded job after repair: unload and reload expected target.
            bootout(host, paths, &plist_path)?;
        }
        bootstrap(host, paths, &plist_path)
    })();

    if was_disabled {
        // Restore startup-at-login intent without implying stop.
        if let Err(disable_err) = apply_startup_override(host, paths, false) {
            return Err(match result {
                Ok(()) => disable_err,
                Err(start_err) => ServiceError::RollbackFailed {
                    during: start_err.to_string(),
                    detail: format!("restore disabled intent: {disable_err}"),
                },
            });
        }
    }

    result
}

pub fn stop<H: HostCommand>(host: &H, paths: &ServicePaths) -> Result<(), ServiceError> {
    let plist_path = ensure_plist_installed(paths)?;
    match bootout(host, paths, &plist_path) {
        Ok(()) => Ok(()),
        Err(err) => {
            if !service_loaded(host, paths)? {
                Ok(())
            } else {
                Err(err)
            }
        }
    }
}

pub fn restart<H: HostCommand>(host: &H, paths: &ServicePaths) -> Result<(), ServiceError> {
    refuse_if_legacy_root(paths)?;
    // Propagate stop failures — do not report successful restart after a failed stop.
    stop(host, paths)?;
    start(host, paths)
}

pub fn set_startup_at_login<H: HostCommand>(
    host: &H,
    paths: &ServicePaths,
    enabled: bool,
) -> Result<(), ServiceError> {
    let _ = ensure_plist_installed(paths)?;
    // launchctl enable/disable only — no bootstrap/bootout (preserves running).
    apply_startup_override(host, paths, enabled)
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

pub(crate) fn refuse_if_legacy_root(paths: &ServicePaths) -> Result<(), ServiceError> {
    if paths.legacy_root_present() {
        return Err(ServiceError::LegacyRootConflict);
    }
    Ok(())
}

/// Restore prior enable + loaded/running after a failed mutation (best-effort, errors returned).
pub(crate) fn restore_login_and_runtime<H: HostCommand>(
    host: &H,
    paths: &ServicePaths,
    plist_path: &Path,
    prior: &ServiceFacts,
) -> Result<(), ServiceError> {
    apply_startup_override(host, paths, prior.enabled_at_login)?;
    let loaded_now = service_loaded(host, paths)?;
    if prior.loaded || prior.running {
        if !loaded_now {
            bootstrap(host, paths, plist_path)?;
        }
        if !prior.enabled_at_login {
            apply_startup_override(host, paths, false)?;
        }
    } else if loaded_now && !prior.loaded {
        bootout(host, paths, plist_path)?;
        apply_startup_override(host, paths, prior.enabled_at_login)?;
    }
    Ok(())
}

fn apply_startup_override<H: HostCommand>(
    host: &H,
    paths: &ServicePaths,
    enabled: bool,
) -> Result<(), ServiceError> {
    let target = paths.launchd_service_target();
    if enabled {
        run_launchctl(host, &["enable".into(), target])
    } else {
        run_launchctl(host, &["disable".into(), target])
    }
}

fn ensure_plist_installed(paths: &ServicePaths) -> Result<PathBuf, ServiceError> {
    let plist_path = paths.launch_agent_plist();
    if !plist_path.is_file() {
        return Err(ServiceError::PathInvalid {
            path: plist_path,
            reason: "managed LaunchAgent plist is not installed".into(),
        });
    }
    Ok(plist_path)
}

fn bootstrap<H: HostCommand>(
    host: &H,
    paths: &ServicePaths,
    plist_path: &Path,
) -> Result<(), ServiceError> {
    let domain = paths.launchd_gui_domain();
    let plist = plist_path.display().to_string();
    match run_launchctl(host, &["bootstrap".into(), domain.clone(), plist.clone()]) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Documented fallback when bootstrap is unavailable.
            run_launchctl(host, &["load".into(), plist])
        }
    }
}

fn bootout<H: HostCommand>(
    host: &H,
    paths: &ServicePaths,
    plist_path: &Path,
) -> Result<(), ServiceError> {
    let domain = paths.launchd_gui_domain();
    let plist = plist_path.display().to_string();
    match run_launchctl(host, &["bootout".into(), domain, plist.clone()]) {
        Ok(()) => Ok(()),
        Err(_) => run_launchctl(host, &["unload".into(), plist]),
    }
}

fn service_loaded<H: HostCommand>(host: &H, paths: &ServicePaths) -> Result<bool, ServiceError> {
    let output = host.run(&launchctl_spec(&[
        "print".into(),
        paths.launchd_service_target(),
    ]))?;
    if output.timed_out {
        return Err(ServiceError::TimedOut {
            program: "launchctl".into(),
            args: vec!["print".into(), paths.launchd_service_target()],
        });
    }
    Ok(output.status == 0)
}

fn loaded_matches_disk<H: HostCommand>(
    host: &H,
    paths: &ServicePaths,
    plist_path: &Path,
) -> Result<bool, ServiceError> {
    let content = fs::read_to_string(plist_path)
        .map_err(|err| ServiceError::io(format!("read {}", plist_path.display()), err))?;
    let inspection = inspect_launch_agent_plist(&content);
    let output = host.run(&launchctl_spec(&[
        "print".into(),
        paths.launchd_service_target(),
    ]))?;
    if output.timed_out {
        return Err(ServiceError::TimedOut {
            program: "launchctl".into(),
            args: vec!["print".into(), paths.launchd_service_target()],
        });
    }
    if output.status != 0 {
        return Ok(false);
    }
    let body = output.stdout_lossy();
    let Some(bin) = inspection.exec_binary.as_deref() else {
        return Ok(false);
    };
    if !body.contains(bin) {
        return Ok(false);
    }
    if let Some(cfg) = inspection.config_path.as_deref() {
        if !body.contains(cfg) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn is_disabled_in_domain<H: HostCommand>(
    host: &H,
    paths: &ServicePaths,
) -> Result<bool, ServiceError> {
    let disabled = host.run(&launchctl_spec(&[
        "print-disabled".into(),
        paths.launchd_gui_domain(),
    ]))?;
    if disabled.timed_out {
        return Err(ServiceError::TimedOut {
            program: "launchctl".into(),
            args: vec!["print-disabled".into(), paths.launchd_gui_domain()],
        });
    }
    if disabled.status != 0 {
        return Ok(false);
    }
    Ok(disabled
        .stdout_lossy()
        .lines()
        .any(|line| disabled_override_line(line)))
}

/// Match ` "com.clipsync" => true ` (unquoted) and quoted variants.
fn disabled_override_line(line: &str) -> bool {
    if !line.contains(LAUNCH_AGENT_LABEL) {
        return false;
    }
    if let Some(idx) = line.find("=>") {
        let rhs = line[idx + 2..].trim();
        return rhs == "true"
            || rhs.starts_with("true")
            || rhs.starts_with("\"true\"")
            || rhs.starts_with("'true'");
    }
    line.contains("disabled")
}

fn run_launchctl<H: HostCommand>(host: &H, args: &[String]) -> Result<(), ServiceError> {
    let spec = launchctl_spec(args);
    let output = host.run(&spec)?;
    require_success(&spec, output)?;
    Ok(())
}

fn launchctl_spec(args: &[String]) -> CommandSpec {
    CommandSpec::new(tool_program("launchctl"), args.to_vec())
}

#[cfg(test)]
mod unit_tests {
    use super::disabled_override_line;

    #[test]
    fn parses_unquoted_print_disabled_bool() {
        assert!(disabled_override_line("\t\"com.clipsync\" => true"));
        assert!(disabled_override_line("\"com.clipsync\" => true"));
        assert!(!disabled_override_line("\"com.clipsync\" => false"));
        assert!(disabled_override_line("\"com.clipsync\" => \"true\""));
    }
}
