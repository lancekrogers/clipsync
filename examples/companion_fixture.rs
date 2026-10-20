//! Isolated production IPC fixture for real GUI/PTY acceptance. Never uses the
//! personal macOS pasteboard. Linux runs only inside the disposable test desktop.
use anyhow::{bail, Result};
use clipsync::{
    adapters::{get_clipboard_provider, HistoryManager},
    auth::{AuthConfig, KeyPair, KeyType, SshAuthenticator},
    config::Config,
    control,
    sync::SyncEngine,
    transport::{tls, Listener, TransportConfig, TransportManager},
};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(target_os = "macos")]
    if !std::env::var("CLIPSYNC_TEST_PASTEBOARD")
        .is_ok_and(|name| name.starts_with("org.clipsync.test."))
    {
        bail!("Set a unique CLIPSYNC_TEST_PASTEBOARD=org.clipsync.test.<id>");
    }
    #[cfg(target_os = "linux")]
    if std::env::var("CLIPSYNC_DISPOSABLE_DESKTOP").as_deref() != Ok("1") {
        bail!("Run this fixture only in a disposable Linux desktop");
    }
    let dir = tempfile::tempdir()?;
    let key = KeyPair::generate(KeyType::Ed25519)?;
    key.save_to_file(&dir.path().join("identity")).await?;
    let mut config = Config {
        node_id: tls::node_id(&key.public_key()),
        listen_addr: "127.0.0.1:0".into(),
        advertise_name: "Isolated test Mac".into(),
        auth: clipsync::config::AuthConfig {
            ssh_key: dir.path().join("identity"),
            authorized_keys: dir.path().join("authorized_devices"),
        },
        clipboard: clipsync::config::ClipboardConfig {
            history_db: dir.path().join("history.db"),
            history_key: Some(dir.path().join("history.key")),
            ..Default::default()
        },
        ..Default::default()
    };
    let config_path = dir.path().join("config.toml");
    let auth = Arc::new(
        SshAuthenticator::new(AuthConfig {
            private_key_path: config.auth.ssh_key.clone(),
            authorized_keys_path: config.auth.authorized_keys.clone(),
            generate_if_missing: false,
        })
        .await?,
    );
    let transport = Arc::new(TransportManager::with_auth(
        TransportConfig::default(),
        auth.clone(),
        config.node_id,
    ));
    let history = Arc::new(
        HistoryManager::new_with_key_path(
            &config.clipboard.history_db,
            config.clipboard.history_key.as_ref().unwrap(),
        )
        .await?,
    );
    let listener = transport.listener(config.socket_addr()?).await?;
    config.listen_addr = listener.local_addr().to_string();
    std::fs::write(&config_path, toml::to_string(&config)?)?;
    let engine = Arc::new(SyncEngine::without_discovery(
        Arc::new(config.clone()),
        Arc::new(get_clipboard_provider().await?),
        history,
        transport,
    ));
    for sample in [
        "https://example.com/review/clipboard-sync",
        "Review notes\nCheck device recovery after reconnect.\nKeep keyboard navigation clear.",
        "cargo test --locked",
        "teammate@example.com",
        "The latest draft is ready for review. Let me know what you think.",
    ] {
        engine.copy_local(sample.to_owned()).await?;
    }
    let server = control::Server::bind(control::socket_path(&config), &config)?;
    let enrollment = Arc::new(clipsync::enrollment::EnrollmentManager::new(
        Arc::new(config.clone()),
        auth,
    )?);
    enrollment.set_listen_port_override(0).await;
    let target = control::EnrollmentTarget::new(engine.clone(), enrollment);
    println!(
        "{}",
        serde_json::json!({"config":config_path, "socket":control::socket_path(&config), "pid":std::process::id()})
    );
    tokio::select! {
        result = server.run(&target) => result?,
        result = engine.run(listener) => result?,
        _ = tokio::signal::ctrl_c() => {},
    }
    engine.shutdown().await;
    Ok(())
}
