//! Shared desktop-user service operations (launchd / systemd --user).
//!
//! Offline setup and companion clients call these APIs with an injected
//! [`HostCommand`]. Tests must never drive the developer's live service.

mod error;
mod host;
mod linux;
mod macos;
mod paths;

pub use error::ServiceError;
pub use host::{
    require_success, tool_program, CommandOutput, CommandSpec, HostCommand, StdHostCommand,
    DEFAULT_MAX_OUTPUT, DEFAULT_TIMEOUT,
};
pub use paths::{
    atomic_write, ensure_managed_dir, restore_previous, validate_managed_file_path, ServicePaths,
    ServiceTarget, LAUNCH_AGENT_LABEL, LAUNCH_AGENT_PLIST_NAME, UNIT_NAME,
};

use std::path::PathBuf;

/// Which user-service manager owns the daemon on this host overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServicePlatform {
    Launchd,
    SystemdUser,
    Unsupported,
}

/// Observable facts for Overview / doctor (enabled ≠ loaded ≠ running).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceFacts {
    pub unit_installed: bool,
    /// Login/startup intent (launchd enable override / systemd is-enabled).
    pub enabled_at_login: bool,
    /// Job known to the manager (launchd print succeeds / systemd LoadState=loaded).
    pub loaded: bool,
    /// Supervised process actively running.
    pub running: bool,
    pub exec_matches: bool,
    pub config_matches: bool,
    pub legacy_root_present: bool,
    pub binary_path: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub platform: ServicePlatform,
    pub unit_path: Option<PathBuf>,
}

/// Shared service operations parameterized by an injectable host runner.
pub struct ServiceOps<H: HostCommand> {
    host: H,
    paths: ServicePaths,
    platform: ServicePlatform,
}

impl<H: HostCommand> ServiceOps<H> {
    pub fn new(host: H, paths: ServicePaths) -> Self {
        let platform = detect_platform();
        Self {
            host,
            paths,
            platform,
        }
    }

    pub fn with_platform(host: H, paths: ServicePaths, platform: ServicePlatform) -> Self {
        Self {
            host,
            paths,
            platform,
        }
    }

    pub fn paths(&self) -> &ServicePaths {
        &self.paths
    }

    pub fn platform(&self) -> ServicePlatform {
        self.platform
    }

    pub fn inspect(&self) -> Result<ServiceFacts, ServiceError> {
        match self.platform {
            ServicePlatform::SystemdUser => linux::inspect(&self.host, &self.paths),
            ServicePlatform::Launchd => macos::inspect(&self.host, &self.paths),
            ServicePlatform::Unsupported => Err(ServiceError::UnsupportedPlatform),
        }
    }

    pub fn inspect_against(&self, target: &ServiceTarget) -> Result<ServiceFacts, ServiceError> {
        let mut facts = self.inspect()?;
        match self.platform {
            ServicePlatform::SystemdUser => linux::apply_match_flags(&mut facts, target),
            ServicePlatform::Launchd => macos::apply_match_flags(&mut facts, target),
            ServicePlatform::Unsupported => {}
        }
        Ok(facts)
    }

    pub fn install(
        &self,
        target: &ServiceTarget,
        enable_at_login: bool,
    ) -> Result<(), ServiceError> {
        match self.platform {
            ServicePlatform::SystemdUser => {
                linux::install(&self.host, &self.paths, target, enable_at_login)
            }
            ServicePlatform::Launchd => {
                macos::install(&self.host, &self.paths, target, enable_at_login)
            }
            ServicePlatform::Unsupported => Err(ServiceError::UnsupportedPlatform),
        }
    }

    /// Install then start; on start failure restore prior unit bytes and enabled/running state.
    ///
    /// Legacy root is refused **before** any mutation. Stop/reload/restore failures surface as
    /// [`ServiceError::RollbackFailed`] rather than silent success.
    pub fn install_and_start(
        &self,
        target: &ServiceTarget,
        enable_at_login: bool,
    ) -> Result<(), ServiceError> {
        let unit_path = match self.platform {
            ServicePlatform::SystemdUser => self.paths.systemd_user_unit(),
            ServicePlatform::Launchd => self.paths.launch_agent_plist(),
            ServicePlatform::Unsupported => return Err(ServiceError::UnsupportedPlatform),
        };

        // Refuse competing root service before writing or starting.
        match self.platform {
            ServicePlatform::SystemdUser => linux::refuse_if_legacy_root(&self.paths)?,
            ServicePlatform::Launchd => macos::refuse_if_legacy_root(&self.paths)?,
            ServicePlatform::Unsupported => {}
        }

        let prior = self.inspect().unwrap_or(ServiceFacts {
            unit_installed: unit_path.is_file(),
            enabled_at_login: false,
            loaded: false,
            running: false,
            exec_matches: false,
            config_matches: false,
            legacy_root_present: self.paths.legacy_root_present(),
            binary_path: None,
            config_path: None,
            platform: self.platform,
            unit_path: Some(unit_path.clone()),
        });

        let previous =
            if unit_path.is_file() {
                Some(std::fs::read(&unit_path).map_err(|err| {
                    ServiceError::io(format!("read {}", unit_path.display()), err)
                })?)
            } else {
                None
            };

        self.install(target, enable_at_login)?;
        if let Err(start_err) = self.start() {
            return Err(self.rollback_after_failed_start(
                start_err,
                &unit_path,
                previous.as_deref(),
                &prior,
            ));
        }
        Ok(())
    }

    fn rollback_after_failed_start(
        &self,
        start_err: ServiceError,
        unit_path: &std::path::Path,
        previous: Option<&[u8]>,
        prior: &ServiceFacts,
    ) -> ServiceError {
        let mut details = Vec::new();

        if let Err(e) = self.stop() {
            details.push(format!("stop: {e}"));
        }

        if let Err(e) = restore_previous(unit_path, previous) {
            details.push(format!("restore unit: {e}"));
            return ServiceError::RollbackFailed {
                during: start_err.to_string(),
                detail: details.join("; "),
            };
        }

        match self.platform {
            ServicePlatform::SystemdUser => {
                if let Err(e) =
                    linux::restore_prior_state(&self.host, prior.enabled_at_login, prior.running)
                {
                    details.push(format!("restore linux state: {e}"));
                }
            }
            ServicePlatform::Launchd => {
                if let Err(e) =
                    macos::restore_login_and_runtime(&self.host, &self.paths, unit_path, prior)
                {
                    details.push(format!("restore launchd state: {e}"));
                }
            }
            ServicePlatform::Unsupported => {}
        }

        if details.is_empty() {
            start_err
        } else {
            ServiceError::RollbackFailed {
                during: start_err.to_string(),
                detail: details.join("; "),
            }
        }
    }

    pub fn start(&self) -> Result<(), ServiceError> {
        match self.platform {
            ServicePlatform::SystemdUser => linux::start(&self.host, &self.paths),
            ServicePlatform::Launchd => macos::start(&self.host, &self.paths),
            ServicePlatform::Unsupported => Err(ServiceError::UnsupportedPlatform),
        }
    }

    pub fn stop(&self) -> Result<(), ServiceError> {
        match self.platform {
            ServicePlatform::SystemdUser => linux::stop(&self.host, &self.paths),
            ServicePlatform::Launchd => macos::stop(&self.host, &self.paths),
            ServicePlatform::Unsupported => Err(ServiceError::UnsupportedPlatform),
        }
    }

    pub fn restart(&self) -> Result<(), ServiceError> {
        match self.platform {
            ServicePlatform::SystemdUser => linux::restart(&self.host, &self.paths),
            ServicePlatform::Launchd => macos::restart(&self.host, &self.paths),
            ServicePlatform::Unsupported => Err(ServiceError::UnsupportedPlatform),
        }
    }

    /// Toggle login/startup without implying start or stop.
    pub fn set_startup_at_login(&self, enabled: bool) -> Result<(), ServiceError> {
        match self.platform {
            ServicePlatform::SystemdUser => {
                linux::set_startup_at_login(&self.host, &self.paths, enabled)
            }
            ServicePlatform::Launchd => {
                macos::set_startup_at_login(&self.host, &self.paths, enabled)
            }
            ServicePlatform::Unsupported => Err(ServiceError::UnsupportedPlatform),
        }
    }
}

fn detect_platform() -> ServicePlatform {
    #[cfg(target_os = "linux")]
    {
        ServicePlatform::SystemdUser
    }
    #[cfg(target_os = "macos")]
    {
        ServicePlatform::Launchd
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        ServicePlatform::Unsupported
    }
}
