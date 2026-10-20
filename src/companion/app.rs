//! Shared offline setup/service coordinator. All filesystem and host operations
//! run off the renderer thread and require an explicit intent after inspection.
use super::{
    client::{Controller, Intent},
    model::{display_text, Route},
};
use crate::{
    config::{
        resolution::{resolve_config, ExplicitKind, ResolveOutcome},
        Config,
    },
    service::{ServiceOps, ServicePaths, ServiceTarget, StdHostCommand},
    setup::bootstrap::{self, BootstrapPaths},
};
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupState {
    Checking,
    Fresh,
    ChooseConfiguration,
    Ready,
    NeedsRepair,
}
#[derive(Clone, Debug)]
pub struct SetupView {
    pub state: SetupState,
    pub config_path: Option<PathBuf>,
    pub choices: Vec<PathBuf>,
    pub daemon_binary: PathBuf,
    pub message: String,
    pub warnings: Vec<String>,
    pub pending: bool,
    pub installed: bool,
    pub running: bool,
    pub startup: bool,
    pub service_error: Option<String>,
}
#[derive(Clone, Debug)]
pub enum SetupIntent {
    Inspect,
    Choose(PathBuf),
    Create,
    Install { startup: bool },
    Start,
    Stop,
    Restart,
    SetStartup(bool),
}
struct Outcome {
    view: SetupView,
    config: Option<Config>,
}
struct Reply {
    outcome: Outcome,
    error: Option<String>,
}

pub struct Companion {
    pub route: Route,
    pub setup: SetupView,
    pub client: Option<Controller>,
    sender: mpsc::SyncSender<SetupIntent>,
    replies: mpsc::Receiver<Reply>,
    last_inspect: Instant,
    config_stamp: Option<String>,
}
impl Companion {
    pub fn new(
        config_path: Option<PathBuf>,
        daemon_binary: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let paths = BootstrapPaths::from_process_env()?;
        let binary = daemon_binary.unwrap_or_else(default_daemon_binary);
        Self::with_paths(paths, config_path, binary)
    }
    pub fn with_paths(
        paths: BootstrapPaths,
        config_path: Option<PathBuf>,
        binary: PathBuf,
    ) -> anyhow::Result<Self> {
        let (sender, jobs) = mpsc::sync_channel(4);
        let (responses, replies) = mpsc::sync_channel(4);
        let setup = initial_view(binary.clone());
        std::thread::Builder::new().name("clipsync-companion-setup".into()).spawn(move || {
            let mut explicit = config_path;
            let service = ServiceOps::new(StdHostCommand, ServicePaths::new(
                paths.home.clone(),
                paths.xdg_config_home.clone().unwrap_or_else(|| paths.home.join(".config")),
                unsafe { libc::geteuid() },
            ));
            while let Ok(intent) = jobs.recv() {
                let action = (|| -> anyhow::Result<()> {
                    if matches!(&intent, SetupIntent::Start | SetupIntent::Stop | SetupIntent::Restart | SetupIntent::SetStartup(_)) {
                        let selected = inspect(&paths, explicit.clone(), &binary);
                        let facts = service.inspect()?;
                        anyhow::ensure!(selected.view.state == SetupState::Ready && facts.unit_installed,
                            "Choose a valid setup and install its user service first.");
                        anyhow::ensure!(facts.config_path == selected.view.config_path,
                            "The installed service uses a different configuration. Review and repair it first.");
                        anyhow::ensure!(facts.binary_path.as_ref() == Some(&binary),
                            "The installed service uses a different daemon binary. Review and repair it first.");
                    }
                    match intent {
                        SetupIntent::Inspect => {},
                        SetupIntent::Choose(path) => { explicit = Some(path); },
                        SetupIntent::Create => {
                            if explicit.is_some() { anyhow::bail!("Repair the explicitly selected configuration before creating a setup."); }
                            let result = bootstrap::fresh_setup(&paths)?;
                            explicit = Some(result.config_path);
                        },
                        SetupIntent::Install { startup } => {
                            let report = inspect(&paths, explicit.clone(), &binary);
                            let config = report.view.config_path.ok_or_else(|| anyhow::anyhow!("Choose a valid setup first."))?;
                            if report.view.state != SetupState::Ready { anyhow::bail!("Repair the selected setup before installing its service."); }
                            bootstrap::adopt_existing(&paths, &config)?;
                            service.install_and_start(&ServiceTarget { binary: binary.clone(), config }, startup)?;
                        },
                        SetupIntent::Start => service.start()?,
                        SetupIntent::Stop => service.stop()?,
                        SetupIntent::Restart => service.restart()?,
                        SetupIntent::SetStartup(enabled) => service.set_startup_at_login(enabled)?,
                    }
                    Ok(())
                })();
                let mut outcome = inspect(&paths, explicit.clone(), &binary);
                match service.inspect() {
                    Ok(facts) => {
                        outcome.view.installed = facts.unit_installed;
                        outcome.view.running = facts.running;
                        outcome.view.startup = facts.enabled_at_login;
                        if facts.legacy_root_present { outcome.view.service_error = Some("A system-wide ClipSync service exists. Disable it before starting the user service.".into()); }
                        if let (Some(selected), Some(managed)) = (&outcome.view.config_path, &facts.config_path) {
                            if selected != managed { outcome.view.service_error = Some("The installed service uses a different configuration. Review the selected path before repairing the service.".into()); }
                        }
                    }
                    Err(error) => outcome.view.service_error = Some(display_text(&error.to_string(), false)),
                }
                if responses.send(Reply { outcome, error: action.err().map(|error| display_text(&error.to_string(), false)) }).is_err() { break; }
            }
        })?;
        let mut app = Self {
            route: Route::Overview,
            setup,
            client: None,
            sender,
            replies,
            last_inspect: Instant::now(),
            config_stamp: None,
        };
        app.setup_intent(SetupIntent::Inspect);
        Ok(app)
    }
    pub fn setup_intent(&mut self, intent: SetupIntent) {
        if self.setup.pending {
            return;
        }
        match self.sender.try_send(intent) {
            Ok(()) => {
                self.setup.pending = true;
                self.last_inspect = Instant::now();
            }
            Err(_) => self.setup.service_error = Some("Setup is busy. Try again.".into()),
        }
    }
    pub fn navigate(&mut self, route: Route) {
        self.route = route;
        if route == Route::Settings {
            self.setup_intent(SetupIntent::Inspect);
        }
        if let Some(client) = &mut self.client {
            client.dispatch(Intent::Navigate(route));
        }
    }
    pub fn tick(&mut self) {
        while let Ok(reply) = self.replies.try_recv() {
            let changed = self.setup.config_path != reply.outcome.view.config_path;
            self.setup = reply.outcome.view;
            self.setup.pending = false;
            if let Some(error) = reply.error {
                self.setup.service_error = Some(error);
            }
            if let Some(config) = reply.outcome.config {
                let stamp = connection_stamp(&config);
                if self.client.is_none() || changed || self.config_stamp != stamp {
                    self.config_stamp = stamp;
                    match Controller::connect(config) {
                        Ok(mut client) => {
                            client.dispatch(Intent::Navigate(self.route));
                            self.client = Some(client);
                        }
                        Err(error) => self.setup.service_error = Some(error.to_string()),
                    }
                }
            } else {
                self.client = None;
                self.config_stamp = None;
            }
        }
        if let Some(client) = &mut self.client {
            client.tick();
        }
        if !self.setup.pending && self.last_inspect.elapsed() >= Duration::from_secs(10) {
            self.setup_intent(SetupIntent::Inspect);
        }
    }
}
fn initial_view(binary: PathBuf) -> SetupView {
    SetupView {
        state: SetupState::Checking,
        config_path: None,
        choices: Vec::new(),
        daemon_binary: binary,
        message: "Checking your ClipSync setup…".into(),
        warnings: Vec::new(),
        pending: false,
        installed: false,
        running: false,
        startup: false,
        service_error: None,
    }
}
fn inspect(paths: &BootstrapPaths, explicit: Option<PathBuf>, binary: &PathBuf) -> Outcome {
    let mut view = initial_view(binary.clone());
    let resolution = resolve_config(
        &paths.to_resolve_paths(),
        explicit.map(|path| (path, ExplicitKind::Api)),
    );
    view.warnings = resolution
        .warnings
        .iter()
        .map(|issue| display_text(&issue.message, false))
        .collect();
    let mut config = None;
    match resolution.outcome {
        Ok(ResolveOutcome::FreshEligible { proposed_path }) => {
            view.state = SetupState::Fresh;
            view.config_path = Some(proposed_path);
            view.message = "Set up ClipSync on this computer. A dedicated device identity will be created; existing SSH access stays separate.".into();
        }
        Ok(ResolveOutcome::Conflict { candidates }) => {
            view.state = SetupState::ChooseConfiguration;
            view.choices = candidates
                .into_iter()
                .map(|candidate| candidate.path)
                .collect();
            view.message = "More than one ClipSync configuration was found. Choose the installation you want to use; the other files will be preserved.".into();
        }
        Ok(ResolveOutcome::Selected {
            path,
            config: selected,
            ..
        }) => {
            view.config_path = Some(path.clone());
            match bootstrap::validate_adoption(paths, &path) {
                Ok(report) if report.blocked.is_none() => {
                    view.state = SetupState::Ready;
                    view.message =
                        "Your existing identity, trusted devices and history are preserved.".into();
                    view.warnings.extend(
                        report
                            .warnings
                            .into_iter()
                            .map(|warning| display_text(&warning, false)),
                    );
                    config = Some(selected);
                }
                Ok(report) => {
                    view.state = SetupState::NeedsRepair;
                    view.message = report.warnings.join("\n");
                }
                Err(error) => {
                    view.state = SetupState::NeedsRepair;
                    view.message = display_text(&error.to_string(), false);
                }
            }
        }
        Err(issue) => {
            view.state = SetupState::NeedsRepair;
            view.message = display_text(&issue.message, false);
        }
    }
    Outcome { view, config }
}
fn default_daemon_binary() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if exe.file_name().is_some_and(|name| name == "clipsync") {
            return exe;
        }
        if let Some(parent) = exe.parent() {
            let sibling = parent.join("clipsync");
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".local/bin/clipsync")
}

fn connection_stamp(config: &Config) -> Option<String> {
    // Old config files omit node_id; deserialization generates a fresh UUID.
    // The daemon derives its actual identity from the key. Do not reset the
    // user’s selection every time the same legacy file is inspected.
    let mut stable = config.clone();
    stable.node_id = uuid::Uuid::nil();
    toml::to_string(&stable).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_generated_ids_do_not_reset_the_companion_connection() {
        let first = Config::default();
        let mut again = first.clone();
        again.node_id = uuid::Uuid::new_v4();
        assert_eq!(connection_stamp(&first), connection_stamp(&again));
        again.listen_addr = "127.0.0.1:8489".into();
        assert_ne!(connection_stamp(&first), connection_stamp(&again));
    }
}
