mod support;
use clipsync::{
    adapters::{ClipboardProviderWrapper, HistoryManager},
    companion::{
        client::{Controller, Intent},
        model::{Connection, Route},
    },
    config::Config,
    control,
    sync::SyncEngine,
    transport::{TransportConfig, TransportManager},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use support::{Clipboard, Identity, Task};

async fn wait(client: &mut Controller, predicate: impl Fn(&Controller) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        client.tick();
        if predicate(client) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "client did not reach expected state: {:?}",
            client.view
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn two_companions_reconcile_pause_and_copy_by_stable_history_id() {
    let identity = Identity::new().await;
    let mut config = Config::default();
    config.node_id = identity.id();
    config.auth.ssh_key = identity.dir.path().join("identity");
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    config.clipboard.history_key = Some(identity.dir.path().join("history.key"));
    let clipboard = Clipboard::default();
    let history = Arc::new(
        HistoryManager::new_with_key_path(
            &config.clipboard.history_db,
            config.clipboard.history_key.as_ref().unwrap(),
        )
        .await
        .unwrap(),
    );
    let engine = Arc::new(SyncEngine::without_discovery(
        Arc::new(config.clone()),
        Arc::new(ClipboardProviderWrapper::new(Box::new(clipboard.clone()))),
        history,
        Arc::new(TransportManager::with_auth(
            TransportConfig::default(),
            identity.auth.clone(),
            identity.id(),
        )),
    ));
    engine
        .copy_local("Saved review note\nSecond line".into())
        .await
        .unwrap();
    let server = control::Server::bind(control::socket_path(&config), &config).unwrap();
    let target = engine.clone();
    let task = Task(tokio::spawn(
        async move { server.run(target.as_ref()).await },
    ));
    let mut desktop = Controller::connect(config.clone()).unwrap();
    let mut terminal = Controller::connect(config).unwrap();
    wait(&mut desktop, |client| client.view.can_act()).await;
    wait(&mut terminal, |client| client.view.can_act()).await;
    desktop.dispatch(Intent::SetPaused(true));
    wait(&mut desktop, |client| {
        client.view.connection == Connection::Paused && !client.view.action_pending
    })
    .await;
    wait(&mut terminal, |client| {
        client.view.connection == Connection::Paused
    })
    .await;
    terminal.dispatch(Intent::Navigate(Route::History));
    wait(&mut terminal, |client| !client.view.history.is_empty()).await;
    let id = terminal.view.history[0].id;
    terminal.dispatch(Intent::Select(id));
    wait(&mut terminal, |client| client.view.detail.is_some()).await;
    assert_eq!(
        terminal.view.detail.as_deref(),
        Some("Saved review note\nSecond line")
    );
    clipboard.copy("something else").await;
    terminal.dispatch(Intent::CopySelected);
    wait(&mut terminal, |client| {
        !client.view.action_pending
            && client
                .view
                .notice
                .as_deref()
                .is_some_and(|text| text.contains("Copied locally"))
    })
    .await;
    assert_eq!(
        *clipboard.text.lock().await,
        "Saved review note\nSecond line"
    );
    terminal.dispatch(Intent::Search("does-not-match".into()));
    wait(&mut terminal, |client| {
        !client.view.history_loading && client.view.history.is_empty()
    })
    .await;
    assert_eq!(terminal.view.query, "does-not-match");
    assert!(terminal.view.selected.is_none());
    assert!(terminal.view.detail.is_none());
    drop(task);
    terminal.dispatch(Intent::Refresh);
    wait(&mut terminal, |client| {
        client.view.connection == Connection::Unavailable
    })
    .await;
    assert!(!terminal.view.can_act());
}

struct OldDaemon;
#[async_trait::async_trait]
impl control::Target for OldDaemon {
    async fn peers(&self) -> Vec<clipsync::adapters::Peer> {
        Vec::new()
    }
    async fn sync(&self) -> anyhow::Result<usize> {
        Ok(0)
    }
    async fn copy_text(&self, _: String) -> anyhow::Result<usize> {
        panic!("old daemon must not receive companion actions")
    }
}
#[tokio::test]
async fn old_daemon_requires_upgrade_before_actions() {
    let identity = Identity::new().await;
    let mut config = Config::default();
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    let server = control::Server::bind(control::socket_path(&config), &config).unwrap();
    let _task = Task(tokio::spawn(async move { server.run(&OldDaemon).await }));
    let mut client = Controller::connect(config).unwrap();
    client.dispatch(Intent::Navigate(Route::History));
    wait(&mut client, |client| {
        client.view.connection == Connection::UpgradeRequired
    })
    .await;
    assert!(!client.view.can_act());
    client.dispatch(Intent::SetPaused(true));
    assert!(!client.view.action_pending);
    assert!(!client.view.history_loading);
}
