//! Companion-oriented control fixtures: feature gates, dual-client pause visibility,
//! and stale generation handling. Uses mocked clipboard + temp history only.
mod support;
use clipsync::{
    adapters::{ClipboardProviderWrapper, HistoryManager},
    config::Config,
    control::{self, Command, FEATURE_PAUSE},
    sync::SyncEngine,
    transport::{TransportConfig, TransportManager},
};
use std::sync::Arc;
use support::*;

async fn engine_on_socket(
    identity: &Identity,
) -> (
    Config,
    Arc<SyncEngine>,
    Clipboard,
    std::path::PathBuf,
    usize,
    Task<anyhow::Result<()>>,
) {
    let mut config = Config::default();
    config.node_id = identity.id();
    config.listen_addr = "127.0.0.1:0".into();
    config.auth.ssh_key = identity.dir.path().join("identity");
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    let clipboard = Clipboard::default();
    let transport = Arc::new(TransportManager::with_auth(
        TransportConfig::default(),
        identity.auth.clone(),
        identity.id(),
    ));
    let history = Arc::new(
        HistoryManager::new_with_key_path(
            &identity.dir.path().join("history.db"),
            &identity.dir.path().join("history.key"),
        )
        .await
        .unwrap(),
    );
    let engine = Arc::new(SyncEngine::without_discovery(
        Arc::new(config.clone()),
        Arc::new(ClipboardProviderWrapper::new(Box::new(clipboard.clone()))),
        history,
        transport,
    ));
    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let run_engine = engine.clone();
    let task = Task(tokio::spawn(async move {
        server.run(run_engine.as_ref()).await
    }));
    (config, engine, clipboard, path, max_frame, task)
}

/// Minimal companion controller fixture: tracks last applied generation + features.
#[derive(Default)]
struct CompanionView {
    last_generation: Option<u64>,
    instance_id: Option<uuid::Uuid>,
    paused: bool,
    features_ok: bool,
    stale_ignored: u32,
}

impl CompanionView {
    fn apply_status(&mut self, response: &control::Response) {
        let features = response.features.as_deref().unwrap_or(&[]);
        self.features_ok = features.iter().any(|f| f == FEATURE_PAUSE)
            && features.iter().any(|f| f == control::FEATURE_HISTORY_READ);
        let Some(instance) = response.instance_id else {
            self.features_ok = false;
            return;
        };
        let Some(generation) = response.state_generation else {
            return;
        };
        if self.instance_id != Some(instance) {
            self.instance_id = Some(instance);
            self.last_generation = None;
        }
        if self.last_generation.is_some_and(|last| generation < last) {
            self.stale_ignored += 1;
            return;
        }
        self.last_generation = Some(generation);
        if let Some(status) = &response.status {
            self.paused = status.paused;
        }
    }
}

#[tokio::test]
async fn feature_less_status_marks_capability_mismatch() {
    // Phase-0 Daemon mock: Status via default control_snapshot has empty features.
    #[derive(Default)]
    struct LegacyDaemon;
    #[async_trait::async_trait]
    impl control::Target for LegacyDaemon {
        async fn peers(&self) -> Vec<clipsync::adapters::Peer> {
            Vec::new()
        }
        async fn sync(&self) -> anyhow::Result<usize> {
            Ok(0)
        }
        async fn copy_text(&self, _text: String) -> anyhow::Result<usize> {
            Ok(0)
        }
    }

    let identity = Identity::new().await;
    let mut config = Config::default();
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let daemon = Arc::new(LegacyDaemon);
    let task = Task(tokio::spawn(
        async move { server.run(daemon.as_ref()).await },
    ));
    let status = control::request(&path, Command::Status, max_frame)
        .await
        .unwrap();
    let mut view = CompanionView::default();
    view.apply_status(&status);
    assert!(!view.features_ok);
    drop(task);
}

#[tokio::test]
async fn dual_client_pause_visible_and_stale_generation_ignored() {
    let identity = Identity::new().await;
    let (_config, _engine, _clipboard, path, max_frame, task) = engine_on_socket(&identity).await;

    let mut gui = CompanionView::default();
    let mut tui = CompanionView::default();

    let s1 = control::request(&path, Command::Status, max_frame)
        .await
        .unwrap();
    gui.apply_status(&s1);
    tui.apply_status(&s1);
    assert!(gui.features_ok);
    assert!(!gui.paused);

    let pause = control::request(&path, Command::SetPause { paused: true }, max_frame)
        .await
        .unwrap();
    gui.apply_status(&pause);
    assert!(gui.paused);
    let gen = gui.last_generation.unwrap();

    let s2 = control::request(&path, Command::Status, max_frame)
        .await
        .unwrap();
    tui.apply_status(&s2);
    assert!(tui.paused);
    assert!(tui.last_generation.unwrap() >= gen);
    assert_eq!(tui.instance_id, gui.instance_id);

    // Simulate a late Status with an older generation for the same instance.
    let mut stale = s2.clone();
    stale.state_generation = Some(0);
    let ignored_before = tui.stale_ignored;
    tui.apply_status(&stale);
    assert_eq!(tui.stale_ignored, ignored_before + 1);
    assert!(tui.paused);
    drop(task);
}

#[tokio::test]
async fn new_instance_id_resets_generation_tracking() {
    let identity = Identity::new().await;
    let (_c, _e, _cb, path, max_frame, task) = engine_on_socket(&identity).await;
    let mut view = CompanionView::default();
    let status = control::request(&path, Command::Status, max_frame)
        .await
        .unwrap();
    view.apply_status(&status);
    let first_instance = view.instance_id;

    // Fabricate a response as if a new daemon process appeared.
    let mut restarted = status.clone();
    restarted.instance_id = Some(uuid::Uuid::new_v4());
    restarted.state_generation = Some(0);
    restarted.status.as_mut().unwrap().paused = false;
    view.apply_status(&restarted);
    assert_ne!(view.instance_id, first_instance);
    assert_eq!(view.last_generation, Some(0));
    assert!(!view.paused);
    drop(task);
}
