mod support;
use clipsync::{
    adapters::{ClipboardProviderWrapper, HistoryManager, Peer},
    config::Config,
    control::{self, Command, Target},
    sync::SyncEngine,
    transport::{TransportConfig, TransportManager},
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use support::*;
#[derive(Default)]
struct Daemon {
    calls: AtomicUsize,
    copy_calls: tokio::sync::Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl Target for Daemon {
    async fn peers(&self) -> Vec<Peer> {
        vec![Peer {
            id: uuid::Uuid::nil(),
            hostname: "loopback peer".into(),
            address: "127.0.0.1:8485".into(),
        }]
    }
    async fn sync(&self) -> anyhow::Result<usize> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(1)
    }
    async fn copy_text(&self, text: String) -> anyhow::Result<usize> {
        self.copy_calls.lock().await.push(text);
        Ok(1)
    }
}
#[tokio::test]
async fn separate_cli_processes_reach_daemon_and_fail_when_absent() {
    let identity = Identity::new().await;
    let mut config = Config::default();
    config.auth.ssh_key = identity.dir.path().join("identity");
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    let config_path = identity.dir.path().join("config.toml");
    std::fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    assert!(control::Server::bind(path.clone(), &config).is_err());
    let daemon = Arc::new(Daemon::default());
    let target = daemon.clone();
    let task = Task(tokio::spawn(
        async move { server.run(target.as_ref()).await },
    ));
    for (command, expected) in [
        ("status", "daemon running"),
        ("peers", "loopback peer"),
        ("sync", "queued for 1"),
    ] {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_clipsync"))
            .args(["--config", config_path.to_str().unwrap(), command])
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(expected),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    assert_eq!(daemon.calls.load(Ordering::SeqCst), 1);
    let mut stream = tokio::net::UnixStream::connect(&path).await.unwrap();
    use tokio::io::AsyncWriteExt;
    stream.write_u32((max_frame + 1) as u32).await.unwrap();
    drop(stream);
    assert!(control::request(&path, control::Command::Status, max_frame)
        .await
        .is_ok());
    drop(task);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_clipsync"))
        .args(["--config", config_path.to_str().unwrap(), "sync"])
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not reachable"));
    #[cfg(target_os = "linux")]
    assert!(stderr.contains("start --foreground"));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cli_copy_reaches_daemon_and_rejects_oversize() {
    let identity = Identity::new().await;
    let mut config = Config::default();
    config.auth.ssh_key = identity.dir.path().join("identity");
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    config.clipboard.max_size = 1024;
    let config_path = identity.dir.path().join("config.toml");
    std::fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    let path = control::socket_path(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let daemon = Arc::new(Daemon::default());
    let target = daemon.clone();
    let task = Task(tokio::spawn(
        async move { server.run(target.as_ref()).await },
    ));
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_clipsync"))
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "copy",
            "daemon clipboard fixture",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Text copied to clipboard"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let copies = daemon.copy_calls.lock().await;
    assert_eq!(copies.as_slice(), ["daemon clipboard fixture"]);
    drop(copies);
    let oversize = "x".repeat(1025);
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_clipsync"))
        .args(["--config", config_path.to_str().unwrap(), "copy", &oversize])
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("size limit"), "{}", stderr);
    assert!(!stderr.contains("start --foreground"), "{}", stderr);
    assert_eq!(daemon.copy_calls.lock().await.len(), 1);
    drop(task);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_clipsync"))
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "copy",
            "unreachable daemon",
        ])
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not reachable"), "{}", stderr);
    assert!(stderr.contains("start --foreground"), "{}", stderr);
}

#[tokio::test]
async fn control_copy_roundtrip_maximally_escaped_payload() {
    let identity = Identity::new().await;
    let mut config = Config::default();
    config.node_id = identity.id();
    config.listen_addr = "127.0.0.1:0".into();
    config.auth.ssh_key = identity.dir.path().join("identity");
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    config.clipboard.max_size = 16 * 1024;
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
    let task = Task(tokio::spawn(
        async move { server.run(engine.as_ref()).await },
    ));
    let text = "\u{1}".repeat(config.clipboard.max_size);
    control::request(&path, Command::Copy { text: text.clone() }, max_frame)
        .await
        .unwrap();
    assert_eq!(*clipboard.text.lock().await, text);
    assert_eq!(clipboard.writes.load(Ordering::SeqCst), 1);
    let oversize = "x".repeat(config.clipboard.max_size + 1);
    assert!(
        control::request(&path, Command::Copy { text: oversize }, max_frame)
            .await
            .is_err()
    );
    assert_eq!(clipboard.writes.load(Ordering::SeqCst), 1);
    drop(task);
}

async fn engine_fixture(
    identity: &Identity,
    clipboard: &Clipboard,
) -> (Config, Arc<SyncEngine>, Arc<HistoryManager>) {
    let mut config = Config::default();
    config.node_id = identity.id();
    config.listen_addr = "127.0.0.1:0".into();
    config.auth.ssh_key = identity.dir.path().join("identity");
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    config.clipboard.max_size = 64 * 1024;
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
        history.clone(),
        transport,
    ));
    (config, engine, history)
}

#[tokio::test]
async fn status_advertises_features_instance_and_generation() {
    let identity = Identity::new().await;
    let clipboard = Clipboard::default();
    let (config, engine, _) = engine_fixture(&identity, &clipboard).await;
    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let task = Task(tokio::spawn(
        async move { server.run(engine.as_ref()).await },
    ));
    let status = control::request(&path, Command::Status, max_frame)
        .await
        .unwrap();
    let features = status.features.expect("features");
    for required in [
        control::FEATURE_PAUSE,
        control::FEATURE_HISTORY_READ,
        control::FEATURE_HISTORY_COPY,
        control::FEATURE_DIAGNOSTICS,
    ] {
        assert!(features.iter().any(|f| f == required), "{features:?}");
    }
    assert!(status.instance_id.is_some());
    assert!(status.state_generation.is_some());
    assert!(status.status.as_ref().is_some_and(|s| !s.paused));
    drop(task);
}

#[tokio::test]
async fn set_pause_blocks_sync_and_gates_copy_local_only() {
    let identity = Identity::new().await;
    let clipboard = Clipboard::default();
    let (config, engine, history) = engine_fixture(&identity, &clipboard).await;
    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let task = Task(tokio::spawn(
        async move { server.run(engine.as_ref()).await },
    ));

    let paused = control::request(&path, Command::SetPause { paused: true }, max_frame)
        .await
        .unwrap();
    assert_eq!(paused.pause_applied, Some(true));
    assert!(paused.status.as_ref().unwrap().paused);
    let gen_after_pause = paused.state_generation.unwrap();

    let sync_err = control::request(&path, Command::Sync, max_frame)
        .await
        .unwrap_err()
        .to_string();
    assert!(sync_err.contains("Sync paused"), "{sync_err}");

    let before = history.get_recent_entries(20).await.unwrap().len();
    let copy = control::request(
        &path,
        Command::Copy {
            text: "paused-local-only".into(),
        },
        max_frame,
    )
    .await
    .unwrap();
    assert_eq!(copy.queued, Some(0));
    assert_eq!(copy.pause_applied, Some(true));
    assert_eq!(*clipboard.text.lock().await, "paused-local-only");
    assert_eq!(history.get_recent_entries(20).await.unwrap().len(), before);

    let resumed = control::request(&path, Command::SetPause { paused: false }, max_frame)
        .await
        .unwrap();
    assert!(!resumed.status.as_ref().unwrap().paused);
    assert!(resumed.state_generation.unwrap() > gen_after_pause);
    drop(task);
}

#[tokio::test]
async fn pause_drops_remote_apply_and_clears_pending_without_resume_flush() {
    let identity = Identity::new().await;
    let clipboard = Clipboard::default();
    clipboard.copy("baseline-before-pause").await;
    let (_config, engine, history) = engine_fixture(&identity, &clipboard).await;

    // Seed an outbound pending by copying while running.
    engine
        .copy_local("outbound-pending-seed".into())
        .await
        .unwrap();
    assert!(engine.runtime().await.outbound_pending);

    engine.set_pause(true).await.unwrap();
    assert!(engine.is_paused().await);
    assert!(!engine.runtime().await.outbound_pending);

    let rows_before = history.get_recent_entries(20).await.unwrap().len();
    let writes_before = clipboard.writes.load(Ordering::SeqCst);

    // Fabricate a remote clipboard message; apply must no-op while paused.
    let origin = uuid::Uuid::new_v4();
    let mut remote = clipsync::transport::Message::new(
        clipsync::transport::MessageType::ClipboardData,
        clipsync::transport::MessagePayload::Clipboard(clipsync::transport::ClipboardData {
            format: clipsync::transport::protocol::ClipboardFormat::Text,
            data: b"remote-during-pause".to_vec(),
            compression: None,
            checksum: clipsync::history::encryption::Encryptor::compute_checksum(
                b"remote-during-pause",
            ),
            metadata: Default::default(),
        }),
    );
    remote.source_peer_id = Some(origin);
    remote.correlation_id = Some(uuid::Uuid::new_v4());
    remote.sequence = chrono::Utc::now().timestamp_millis().max(0) as u64 + 1_000;
    engine.apply_remote(remote).await.unwrap();

    assert_eq!(clipboard.writes.load(Ordering::SeqCst), writes_before);
    assert_eq!(
        history.get_recent_entries(20).await.unwrap().len(),
        rows_before
    );
    assert_eq!(*clipboard.text.lock().await, "outbound-pending-seed");

    // Change clipboard only while paused; resume must baseline without emitting pending.
    clipboard.copy("pause-window-only").await;
    engine.set_pause(false).await.unwrap();
    assert!(!engine.is_paused().await);
    assert!(!engine.runtime().await.outbound_pending);
}

#[tokio::test]
async fn history_list_search_get_copy_bounds_and_preview_only() {
    let identity = Identity::new().await;
    let clipboard = Clipboard::default();
    let (config, engine, history) = engine_fixture(&identity, &clipboard).await;
    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let task = Task(tokio::spawn(
        async move { server.run(engine.as_ref()).await },
    ));

    for i in 0..5 {
        control::request(
            &path,
            Command::Copy {
                text: format!("history-fixture-{i}-needle"),
            },
            max_frame,
        )
        .await
        .unwrap();
    }

    let list = control::request(&path, Command::HistoryList { limit: 100 }, max_frame)
        .await
        .unwrap();
    let page = list.history.as_ref().unwrap();
    assert!(page.total_returned <= 20);
    assert!(page.entries.len() <= 20);
    assert!(page
        .entries
        .iter()
        .all(|e| e.preview.chars().count() <= 240));
    assert!(page.query.is_none());

    let search = control::request(
        &path,
        Command::HistorySearch {
            query: "needle".into(),
            limit: 20,
        },
        max_frame,
    )
    .await
    .unwrap();
    assert!(search.history.as_ref().unwrap().total_returned >= 1);
    assert_eq!(
        search.history.as_ref().unwrap().query.as_deref(),
        Some("needle")
    );

    let empty = control::request(
        &path,
        Command::HistorySearch {
            query: "zzz-no-match".into(),
            limit: 20,
        },
        max_frame,
    )
    .await
    .unwrap();
    assert_eq!(empty.history.as_ref().unwrap().total_returned, 0);
    assert!(empty.error.is_none());

    let too_long = "q".repeat(257);
    let qerr = control::request(
        &path,
        Command::HistorySearch {
            query: too_long,
            limit: 20,
        },
        max_frame,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(qerr.contains("History query too long"), "{qerr}");

    let id = page.entries[0].id;
    let detail = control::request(&path, Command::HistoryGet { id }, max_frame)
        .await
        .unwrap();
    let entry = detail.history_entry.as_ref().unwrap();
    assert!(entry.text.contains("needle"));
    assert!(!entry.text.is_empty());

    let missing = control::request(
        &path,
        Command::HistoryGet {
            id: uuid::Uuid::new_v4(),
        },
        max_frame,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(missing.contains("History entry not found"), "{missing}");

    let copied = control::request(&path, Command::HistoryCopy { id }, max_frame)
        .await
        .unwrap();
    assert!(copied.queued.is_some());
    assert_eq!(*clipboard.text.lock().await, entry.text);

    // Pause then HistoryCopy is local-only.
    control::request(&path, Command::SetPause { paused: true }, max_frame)
        .await
        .unwrap();
    let rows = history.get_recent_entries(20).await.unwrap().len();
    let paused_copy = control::request(&path, Command::HistoryCopy { id }, max_frame)
        .await
        .unwrap();
    assert_eq!(paused_copy.queued, Some(0));
    assert_eq!(paused_copy.pause_applied, Some(true));
    assert_eq!(history.get_recent_entries(20).await.unwrap().len(), rows);
    drop(task);
}

#[tokio::test]
async fn diagnostics_export_is_redacted() {
    let identity = Identity::new().await;
    let clipboard = Clipboard::default();
    let (config, engine, _) = engine_fixture(&identity, &clipboard).await;
    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let task = Task(tokio::spawn(
        async move { server.run(engine.as_ref()).await },
    ));
    let sentinel = "SENTel_clipboard_secret_should_not_leak";
    control::request(
        &path,
        Command::Copy {
            text: sentinel.into(),
        },
        max_frame,
    )
    .await
    .unwrap();
    let response = control::request(&path, Command::DiagnosticsExport, max_frame)
        .await
        .unwrap();
    let report = response.diagnostics.as_ref().unwrap();
    let encoded = serde_json::to_string(report).unwrap();
    assert!(!encoded.contains(sentinel));
    assert!(!encoded.contains(identity.dir.path().to_str().unwrap()));
    assert_eq!(report.redaction, "clipsync-diagnostics-v1");
    assert!(report
        .config_labels
        .iter()
        .any(|l| l.starts_with("history_key=")));
    drop(task);
}

#[tokio::test]
async fn unsupported_protocol_version_is_rejected() {
    let identity = Identity::new().await;
    let clipboard = Clipboard::default();
    let (config, engine, _) = engine_fixture(&identity, &clipboard).await;
    let path = control::socket_path(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let task = Task(tokio::spawn(
        async move { server.run(engine.as_ref()).await },
    ));
    let mut stream = tokio::net::UnixStream::connect(&path).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let body = serde_json::to_vec(&serde_json::json!({
        "version": 99,
        "command": "Status"
    }))
    .unwrap();
    stream.write_u32(body.len() as u32).await.unwrap();
    stream.write_all(&body).await.unwrap();
    let len = stream.read_u32().await.unwrap() as usize;
    let mut buf = vec![0; len];
    stream.read_exact(&mut buf).await.unwrap();
    let response: control::Response = serde_json::from_slice(&buf).unwrap();
    assert!(response
        .error
        .as_deref()
        .unwrap()
        .contains("Unsupported control protocol"));
    assert_eq!(
        response.error_code.as_deref(),
        Some(clipsync::control::error_codes::UNSUPPORTED_PROTOCOL)
    );
    drop(task);
}
