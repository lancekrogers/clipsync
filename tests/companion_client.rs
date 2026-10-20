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
    client.dispatch(Intent::Search("secret-clipboard".into()));
    assert!(client.view.history.is_empty());
    assert!(!client.view.history_loading);
    assert!(client
        .view
        .error
        .as_deref()
        .is_some_and(|text| !text.contains("secret-clipboard")));
}

#[tokio::test]
async fn incompatible_response_version_requires_update_instead_of_restart() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let identity = Identity::new().await;
    let mut config = Config::default();
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    let path = control::socket_path(&config);
    // Let the real server establish the isolated private directory, then replace
    // only this fixture socket with an incompatible-version daemon.
    drop(control::Server::bind(path.clone(), &config).unwrap());
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let _task = Task(tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let length = stream.read_u32().await.unwrap() as usize;
            assert!(length < 8192);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).await.unwrap();
            let mut response = control::Response::base();
            response.version = 99;
            let body = serde_json::to_vec(&response).unwrap();
            stream.write_u32(body.len() as u32).await.unwrap();
            stream.write_all(&body).await.unwrap();
        }
    }));
    let mut client = Controller::connect(config).unwrap();
    wait(&mut client, |client| {
        client.view.connection == Connection::UpgradeRequired
    })
    .await;
    assert!(!client.view.can_act());
    assert!(client.view.error.as_deref().unwrap().contains("update"));
    drop(client);
    drop(_task);
    std::fs::remove_file(path).unwrap();
}

fn ready_snapshot(instance: uuid::Uuid) -> control::ControlSnapshot {
    control::ControlSnapshot {
        peers: Vec::new(),
        features: control::engine_features(),
        instance_id: instance,
        state_generation: 1,
        status: control::DaemonStatus {
            paused: false,
            observed_at: "2026-10-20T00:00:00Z".into(),
            connected_peer_count: 0,
            history_available: true,
            node_id: instance.to_string(),
            max_clipboard_bytes: 1024,
            history_capacity: 20,
            outbound_pending: false,
        },
    }
}

struct LeakyHistory {
    instance: uuid::Uuid,
}
#[async_trait::async_trait]
impl control::Target for LeakyHistory {
    async fn peers(&self) -> Vec<clipsync::adapters::Peer> {
        Vec::new()
    }
    async fn sync(&self) -> anyhow::Result<usize> {
        Ok(0)
    }
    async fn copy_text(&self, _: String) -> anyhow::Result<usize> {
        anyhow::bail!("clipboard must not be written by this fixture")
    }
    async fn control_snapshot(&self) -> anyhow::Result<control::ControlSnapshot> {
        Ok(ready_snapshot(self.instance))
    }
    async fn history_list(&self, _: u32) -> anyhow::Result<control::HistoryPage> {
        anyhow::bail!("History unavailable: secret-clipboard /tmp/history.key")
    }
}

#[tokio::test]
async fn history_errors_hide_payloads_and_rows() {
    let identity = Identity::new().await;
    let mut config = Config::default();
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    let server = control::Server::bind(control::socket_path(&config), &config).unwrap();
    let target = LeakyHistory {
        instance: uuid::Uuid::new_v4(),
    };
    let _task = Task(tokio::spawn(async move { server.run(&target).await }));
    let mut client = Controller::connect(config).unwrap();
    wait(&mut client, |client| client.view.can_act()).await;
    client.dispatch(Intent::Navigate(Route::History));
    wait(&mut client, |client| {
        client.view.error.is_some() && !client.view.history_loading
    })
    .await;
    let err = client.view.error.as_deref().unwrap();
    assert!(err.contains("unavailable"), "{err}");
    assert!(!err.contains("secret-clipboard"), "{err}");
    assert!(!err.contains("history.key"), "{err}");
    assert!(client.view.history.is_empty());
    assert!(client.view.detail.is_none());
    assert_eq!(
        client.view.error_code.as_deref(),
        Some(clipsync::control::error_codes::HISTORY_UNAVAILABLE)
    );
}

struct LeakyDiagnostics {
    instance: uuid::Uuid,
}
#[async_trait::async_trait]
impl control::Target for LeakyDiagnostics {
    async fn peers(&self) -> Vec<clipsync::adapters::Peer> {
        Vec::new()
    }
    async fn sync(&self) -> anyhow::Result<usize> {
        Ok(0)
    }
    async fn copy_text(&self, _: String) -> anyhow::Result<usize> {
        anyhow::bail!("clipboard must not be written by this fixture")
    }
    async fn control_snapshot(&self) -> anyhow::Result<control::ControlSnapshot> {
        Ok(ready_snapshot(self.instance))
    }
    async fn history_list(&self, _: u32) -> anyhow::Result<control::HistoryPage> {
        anyhow::bail!("History unavailable: secret-clipboard from /tmp/history.key")
    }
    async fn diagnostics_export(&self) -> anyhow::Result<control::DiagnosticsReport> {
        Ok(control::DiagnosticsReport {
            generated_at: "2026-10-20T00:00:00Z".into(),
            app_version: "0.1.0\u{202e}".into(),
            control_version: 1,
            features: control::engine_features(),
            paused: false,
            connected_peer_count: 0,
            history_available: true,
            history_capacity: 20,
            max_clipboard_bytes: 1024,
            service_mode_hint: "unknown".into(),
            config_labels: vec![
                "history_key=configured".into(),
                "history_key_path=/tmp/history.key".into(),
            ],
            recent_errors: vec![control::DiagnosticError {
                at: "2026-10-20T00:00:01Z".into(),
                code: "history_unavailable".into(),
                message: "copied secret-clipboard from /tmp/history.key".into(),
            }],
            redaction: "clipsync-diagnostics-v1".into(),
        })
    }
}

#[tokio::test]
async fn diagnostics_view_omits_payloads_paths_and_control_marks() {
    let identity = Identity::new().await;
    let mut config = Config::default();
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    let server = control::Server::bind(control::socket_path(&config), &config).unwrap();
    let target = LeakyDiagnostics {
        instance: uuid::Uuid::new_v4(),
    };
    let _task = Task(tokio::spawn(async move { server.run(&target).await }));
    let mut client = Controller::connect(config).unwrap();
    wait(&mut client, |client| client.view.can_act()).await;
    client.dispatch(Intent::Navigate(Route::History));
    wait(&mut client, |client| {
        client.view.error_code.as_deref() == Some("history_unavailable")
    })
    .await;
    client.dispatch(Intent::Diagnostics);
    wait(&mut client, |client| client.view.diagnostics.is_some()).await;
    let report = client.view.diagnostics.as_deref().unwrap();
    assert!(report.contains("clipsync-diagnostics-v1"), "{report}");
    assert!(report.contains("history_key=configured"), "{report}");
    assert!(report.contains("history_unavailable"), "{report}");
    assert!(!report.contains("secret-clipboard"), "{report}");
    assert!(!report.contains("history.key"), "{report}");
    assert!(!report.contains('\u{202e}'), "{report}");
    assert!(!report.contains("BEGIN "), "{report}");
}

struct HeldSearch {
    instance: uuid::Uuid,
    started: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    release: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
#[async_trait::async_trait]
impl control::Target for HeldSearch {
    async fn peers(&self) -> Vec<clipsync::adapters::Peer> {
        Vec::new()
    }
    async fn sync(&self) -> anyhow::Result<usize> {
        Ok(0)
    }
    async fn copy_text(&self, _: String) -> anyhow::Result<usize> {
        anyhow::bail!("clipboard must not be written by this fixture")
    }
    async fn control_snapshot(&self) -> anyhow::Result<control::ControlSnapshot> {
        Ok(ready_snapshot(self.instance))
    }
    async fn history_search(&self, query: String, _: u32) -> anyhow::Result<control::HistoryPage> {
        self.started
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        while !self.release.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        Ok(control::HistoryPage {
            query: Some(query.clone()),
            total_returned: 1,
            truncated: false,
            entries: vec![control::HistoryEntryView {
                id: uuid::Uuid::nil(),
                timestamp: "2026-10-20T00:00:00Z".into(),
                origin_node: self.instance,
                content_type: "text/plain".into(),
                content_chars: query.chars().count() as u32,
                preview: query,
            }],
        })
    }
}

#[tokio::test]
async fn full_search_queue_keeps_the_latest_query() {
    let identity = Identity::new().await;
    let mut config = Config::default();
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let server = control::Server::bind(control::socket_path(&config), &config).unwrap();
    let target = HeldSearch {
        instance: uuid::Uuid::new_v4(),
        started: started.clone(),
        release: release.clone(),
    };
    let _task = Task(tokio::spawn(async move { server.run(&target).await }));
    let mut client = Controller::connect(config).unwrap();
    wait(&mut client, |client| client.view.can_act()).await;
    client.dispatch(Intent::Search("q00".into()));
    let deadline = Instant::now() + Duration::from_secs(2);
    while started.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "search never reached the daemon");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    for index in 1..=20 {
        client.dispatch(Intent::Search(format!("q{index:02}")));
    }
    release.store(true, std::sync::atomic::Ordering::SeqCst);
    wait(&mut client, |client| {
        !client.view.history_loading
            && client.view.query == "q20"
            && client
                .view
                .history
                .first()
                .is_some_and(|row| row.preview == "q20")
    })
    .await;
    assert!(client.view.history.iter().all(|row| row.preview == "q20"));
}

struct HeldDetail {
    instance: uuid::Uuid,
    started: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    release: std::sync::Arc<std::sync::atomic::AtomicBool>,
    first_id: uuid::Uuid,
    latest_id: uuid::Uuid,
}
#[async_trait::async_trait]
impl control::Target for HeldDetail {
    async fn peers(&self) -> Vec<clipsync::adapters::Peer> {
        Vec::new()
    }
    async fn sync(&self) -> anyhow::Result<usize> {
        Ok(0)
    }
    async fn copy_text(&self, _: String) -> anyhow::Result<usize> {
        anyhow::bail!("clipboard must not be written by this fixture")
    }
    async fn control_snapshot(&self) -> anyhow::Result<control::ControlSnapshot> {
        Ok(ready_snapshot(self.instance))
    }
    async fn history_list(&self, _: u32) -> anyhow::Result<control::HistoryPage> {
        let entry = |id, preview: &str| control::HistoryEntryView {
            id,
            timestamp: "2026-10-20T00:00:00Z".into(),
            origin_node: self.instance,
            content_type: "text/plain".into(),
            content_chars: preview.chars().count() as u32,
            preview: preview.into(),
        };
        Ok(control::HistoryPage {
            query: None,
            total_returned: 2,
            truncated: false,
            entries: vec![
                entry(self.first_id, "first-held-detail"),
                entry(self.latest_id, "latest-selected-detail"),
            ],
        })
    }
    async fn history_get(&self, id: uuid::Uuid) -> anyhow::Result<control::HistoryEntryDetail> {
        self.started
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        while !self.release.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let text = if id == self.latest_id {
            "latest-selected-detail"
        } else {
            "first-held-detail"
        };
        Ok(control::HistoryEntryDetail {
            id,
            timestamp: "2026-10-20T00:00:00Z".into(),
            origin_node: self.instance,
            content_type: "text/plain".into(),
            content_chars: text.chars().count() as u32,
            preview: text.into(),
            text: text.into(),
        })
    }
}

#[tokio::test]
async fn parked_latest_detail_survives_status_while_the_queue_is_full() {
    let identity = Identity::new().await;
    let mut config = Config::default();
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let first_id = uuid::Uuid::from_u128(1);
    let latest_id = uuid::Uuid::from_u128(2);
    let server = control::Server::bind(control::socket_path(&config), &config).unwrap();
    let target = HeldDetail {
        instance: uuid::Uuid::new_v4(),
        started: started.clone(),
        release: release.clone(),
        first_id,
        latest_id,
    };
    let _task = Task(tokio::spawn(async move { server.run(&target).await }));
    let mut client = Controller::connect(config).unwrap();
    wait(&mut client, |client| client.view.can_act()).await;
    client.dispatch(Intent::Navigate(Route::History));
    wait(&mut client, |client| !client.view.history.is_empty()).await;
    client.dispatch(Intent::Select(first_id));
    let deadline = Instant::now() + Duration::from_secs(2);
    while started.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "detail never reached the daemon");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // One detail is held in the worker and eight fit in the channel. The rest
    // park the latest selection. Refresh then tries to queue Status.
    for _ in 0..20 {
        client.dispatch(Intent::Select(latest_id));
    }
    assert_eq!(client.view.selected, Some(latest_id));
    assert!(client.view.detail.is_none());
    assert!(client.view.detail_loading);
    client.dispatch(Intent::Refresh);
    assert_eq!(client.view.selected, Some(latest_id));
    assert!(client.view.detail.is_none());
    assert!(
        client.view.detail_loading,
        "status replaced the parked detail and cleared loading"
    );
    release.store(true, std::sync::atomic::Ordering::SeqCst);
    wait(&mut client, |client| {
        !client.view.detail_loading
            && client.view.selected == Some(latest_id)
            && client.view.detail.as_deref() == Some("latest-selected-detail")
    })
    .await;
}
