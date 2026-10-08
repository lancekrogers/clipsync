mod support;
use clipsync::{
    adapters::Peer,
    config::Config,
    control::{self, Target},
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
    let server = control::Server::bind(path.clone()).unwrap();
    assert!(control::Server::bind(path.clone()).is_err());
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
    stream.write_u32(8193).await.unwrap();
    drop(stream);
    assert!(control::request(&path, control::Command::Status)
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("not reachable"));
}
