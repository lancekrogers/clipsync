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
