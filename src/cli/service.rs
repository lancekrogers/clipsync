//! CLI bridge to the same per-user service operations used by both companions.
//!
//! Production entrypoint is [`managed`]. Fixture tests must call [`managed_with`]
//! with an injected [`HostCommand`] and overlay [`ServicePaths`] — never the
//! developer's live launchd/systemd user service.
use crate::service::{HostCommand, ServiceOps, ServicePaths, StdHostCommand};
use anyhow::{ensure, Context, Result};
use std::path::Path;

/// Lifecycle action requested by CLI `start` / `stop` / `restart`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
}

/// Returns false for an unmanaged installation; Linux retains its standalone
/// daemon commands in that case. Never operates on a different selected config.
pub fn managed(action: Action, selected_config: Option<&Path>) -> Result<bool> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let paths = ServicePaths::new(home, xdg, unsafe { libc::geteuid() });
    managed_with(StdHostCommand, paths, action, selected_config)
}

/// Injectable bridge for fixture tests. Uses the same gate as [`managed`]:
/// missing unit → `Ok(false)`; config mismatch → error before mutation;
/// matching unit → [`ServiceOps`] start/stop/restart.
pub fn managed_with<H: HostCommand>(
    host: H,
    paths: ServicePaths,
    action: Action,
    selected_config: Option<&Path>,
) -> Result<bool> {
    if !unit_installed(&paths) {
        return Ok(false);
    }
    let service = ServiceOps::new(host, paths);
    let facts = service.inspect()?;
    let selected =
        selected_config.context("Select the installed service’s configuration with --config")?;
    let managed = facts
        .config_path
        .context("The service has no explicit configuration. Repair it in ClipSync Settings.")?;
    ensure!(
        std::fs::canonicalize(selected)? == std::fs::canonicalize(&managed)?,
        "The installed service uses a different configuration. Select that configuration or repair the service in ClipSync Settings."
    );
    match action {
        Action::Start => service.start()?,
        Action::Stop => service.stop()?,
        Action::Restart => service.restart()?,
    }
    Ok(true)
}

fn unit_installed(paths: &ServicePaths) -> bool {
    #[cfg(target_os = "macos")]
    {
        paths.launch_agent_plist().exists()
    }
    #[cfg(not(target_os = "macos"))]
    {
        paths.systemd_user_unit().exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{CommandOutput, CommandSpec, ServiceError, LAUNCH_AGENT_LABEL};
    use crate::service_install::{render_launch_agent_plist, render_systemd_user_unit_with_config};
    use std::collections::VecDeque;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
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

        fn call_programs(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|c| format!("{} {}", c.program.display(), c.args.join(" ")))
                .collect()
        }

        fn mutation_calls(&self) -> Vec<String> {
            self.call_programs()
                .into_iter()
                .filter(|c| {
                    c.contains(" start ")
                        || c.contains(" stop ")
                        || c.contains(" restart ")
                        || c.contains("bootstrap")
                        || c.contains("bootout")
                        || c.ends_with(" load")
                        || c.contains(" unload")
                })
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

    fn fixture_paths(temp: &Path) -> (ServicePaths, PathBuf, PathBuf) {
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
        fs::write(&config, "node_id = \"cli-bridge-fixture\"\n").unwrap();
        let mut paths = ServicePaths::new(&home, &xdg, 501);
        // Never inspect the developer's real system units.
        paths.legacy_system_units = vec![temp.join("legacy/clipsync.service")];
        (paths, binary, config)
    }

    fn write_managed_unit(paths: &ServicePaths, binary: &Path, config: &Path) {
        let binary_s = binary.to_str().unwrap();
        let config_s = config.to_str().unwrap();
        #[cfg(target_os = "macos")]
        {
            let plist = paths.launch_agent_plist();
            fs::create_dir_all(plist.parent().unwrap()).unwrap();
            let body = render_launch_agent_plist(
                binary_s,
                config_s,
                "/tmp/clipsync-fixture-out.log",
                "/tmp/clipsync-fixture-err.log",
                paths.home.to_str().unwrap(),
                "/usr/bin:/bin",
            )
            .unwrap();
            assert!(body.contains(LAUNCH_AGENT_LABEL));
            fs::write(&plist, body).unwrap();
        }
        #[cfg(not(target_os = "macos"))]
        {
            let unit = paths.systemd_user_unit();
            fs::create_dir_all(unit.parent().unwrap()).unwrap();
            let body = render_systemd_user_unit_with_config(binary_s, Some(config_s)).unwrap();
            fs::write(&unit, body).unwrap();
        }
    }

    #[test]
    fn unmanaged_returns_false_without_host_calls() {
        let temp = tempdir().unwrap();
        let (paths, _binary, config) = fixture_paths(temp.path());
        let host = ScriptedHost::default();
        let handled =
            managed_with(host.clone(), paths, Action::Start, Some(config.as_path())).unwrap();
        assert!(!handled);
        assert!(host.call_programs().is_empty());
    }

    #[test]
    fn mismatched_config_refuses_before_mutation() {
        let temp = tempdir().unwrap();
        let (paths, binary, config) = fixture_paths(temp.path());
        write_managed_unit(&paths, &binary, &config);
        let other = paths.xdg_config_home.join("clipsync/other.toml");
        fs::write(&other, "node_id = \"other\"\n").unwrap();

        let host = ScriptedHost::default();
        #[cfg(target_os = "macos")]
        {
            // inspect: not loaded, not disabled
            host.push_status(1, "");
            host.push_status(0, "");
        }
        #[cfg(not(target_os = "macos"))]
        {
            host.push_status(0, "enabled\n");
            host.push_status(1, "inactive\n");
            host.push_status(0, "loaded\n");
        }

        let err = managed_with(host.clone(), paths, Action::Start, Some(other.as_path()))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("different configuration") || err.contains("Select that configuration"),
            "{err}"
        );
        assert!(
            host.mutation_calls().is_empty(),
            "mismatch must not mutate: {:?}",
            host.call_programs()
        );
    }

    #[test]
    fn matching_config_uses_service_ops() {
        let temp = tempdir().unwrap();
        let (paths, binary, config) = fixture_paths(temp.path());
        write_managed_unit(&paths, &binary, &config);
        let host = ScriptedHost::default();

        #[cfg(target_os = "macos")]
        {
            // managed inspect
            host.push_status(1, ""); // print not loaded
            host.push_status(0, ""); // print-disabled
                                     // start → inspect again
            host.push_status(1, "");
            host.push_status(0, "");
            // bootstrap
            host.push_ok();
        }
        #[cfg(not(target_os = "macos"))]
        {
            // managed inspect
            host.push_status(0, "enabled\n");
            host.push_status(1, "inactive\n");
            host.push_status(0, "loaded\n");
            // start
            host.push_ok();
        }

        let handled =
            managed_with(host.clone(), paths, Action::Start, Some(config.as_path())).unwrap();
        assert!(handled);
        let calls = host.call_programs();
        assert!(
            !calls.is_empty(),
            "matching unit must drive ServiceOps host commands"
        );
        #[cfg(target_os = "macos")]
        {
            assert!(
                calls
                    .iter()
                    .any(|c| c.contains("bootstrap") || c.contains("load")),
                "{calls:?}"
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert!(
                calls.iter().any(|c| c.contains("start clipsync.service")),
                "{calls:?}"
            );
        }
    }

    #[test]
    fn missing_selected_config_errors_when_unit_present() {
        let temp = tempdir().unwrap();
        let (paths, binary, config) = fixture_paths(temp.path());
        write_managed_unit(&paths, &binary, &config);
        let host = ScriptedHost::default();
        #[cfg(target_os = "macos")]
        {
            host.push_status(1, "");
            host.push_status(0, "");
        }
        #[cfg(not(target_os = "macos"))]
        {
            host.push_status(0, "enabled\n");
            host.push_status(1, "inactive\n");
            host.push_status(0, "loaded\n");
        }
        let err = managed_with(host.clone(), paths, Action::Stop, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--config") || err.contains("configuration"),
            "{err}"
        );
        assert!(host.mutation_calls().is_empty());
    }
}
