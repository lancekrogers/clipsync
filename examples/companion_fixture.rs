//! Isolated production IPC fixture for real GUI/PTY acceptance. Never uses the
//! personal macOS pasteboard. Linux runs only inside the disposable test desktop.
use anyhow::{bail, ensure, Result};
use clap::Parser;
use clipsync::{
    adapters::{get_clipboard_provider, HistoryManager},
    auth::{AuthConfig, KeyPair, KeyType, SshAuthenticator},
    config::Config,
    control,
    sync::SyncEngine,
    transport::{tls, Listener, TransportConfig, TransportManager},
};
use std::{path::PathBuf, sync::Arc};

const MARKER: &str = "clipsync-companion-fixture-v1\n";

#[derive(Parser)]
struct Options {
    /// Resume a fixture left by SIGKILL, preserving its identity and history.
    /// Only a directory created by this example is accepted. The caller owns
    /// cleanup of the retained directory after crash/restart acceptance.
    #[arg(long)]
    resume: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::parse();
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
    let fresh = options.resume.is_none();
    let dir = fresh.then(tempfile::tempdir).transpose()?;
    let root = match options.resume {
        Some(path) => path.canonicalize()?,
        None => dir.as_ref().unwrap().path().canonicalize()?,
    };
    let key = if fresh {
        let key = KeyPair::generate(KeyType::Ed25519)?;
        key.save_to_file(&root.join("identity")).await?;
        std::fs::write(root.join(".companion-fixture"), MARKER)?;
        key
    } else {
        ensure!(
            std::fs::read_to_string(root.join(".companion-fixture"))? == MARKER,
            "Not a companion fixture directory"
        );
        // Never accept paths from an arbitrary configuration or follow links to
        // personal identity/history files when resuming a retained fixture.
        for name in [
            "identity",
            "history.db",
            "history.key",
            "config.toml",
            "authorized_devices",
        ] {
            let path = root.join(name);
            match std::fs::symlink_metadata(&path) {
                Ok(meta) => ensure!(
                    meta.is_file() && !meta.file_type().is_symlink(),
                    "Unsafe fixture file: {name}"
                ),
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && name == "authorized_devices" => {}
                Err(error) => return Err(error.into()),
            }
        }
        KeyPair::load_from_file(&root.join("identity")).await?
    };
    let mut config = Config {
        node_id: tls::node_id(&key.public_key()),
        listen_addr: "127.0.0.1:0".into(),
        advertise_name: "Isolated test Mac".into(),
        auth: clipsync::config::AuthConfig {
            ssh_key: root.join("identity"),
            authorized_keys: root.join("authorized_devices"),
        },
        clipboard: clipsync::config::ClipboardConfig {
            history_db: root.join("history.db"),
            history_key: Some(root.join("history.key")),
            ..Default::default()
        },
        ..Default::default()
    };
    let config_path = root.join("config.toml");
    // Acquire the daemon lock before touching retained state. Resuming a live
    // fixture must fail without rewriting its configuration or history.
    let server = control::Server::bind(control::socket_path(&config), &config)?;
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
    ]
    .into_iter()
    .filter(|_| fresh)
    {
        engine.copy_local(sample.to_owned()).await?;
    }
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
