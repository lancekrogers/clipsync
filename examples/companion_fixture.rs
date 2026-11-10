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

/// Exercise the real native write followed by an unresolvable write outcome.
#[cfg(target_os = "macos")]
struct UncertainImageClipboard(Box<dyn clipsync::clipboard::ClipboardProvider>);

#[cfg(target_os = "macos")]
#[async_trait::async_trait]
impl clipsync::clipboard::ClipboardProvider for UncertainImageClipboard {
    async fn get_content(
        &self,
    ) -> Result<clipsync::clipboard::ClipboardContent, clipsync::clipboard::ClipboardError> {
        self.0.get_content().await
    }
    fn enable_image_capture(&self) -> bool {
        self.0.enable_image_capture()
    }
    async fn set_content(
        &self,
        content: &clipsync::clipboard::ClipboardContent,
    ) -> Result<(), clipsync::clipboard::ClipboardError> {
        self.0.set_content(content).await?;
        if content.mime_type == "image/png" {
            return Err(clipsync::clipboard::ClipboardError::WriteOutcomeUnknown(
                "injected test outcome".into(),
            ));
        }
        Ok(())
    }
    async fn clear(&self) -> Result<(), clipsync::clipboard::ClipboardError> {
        self.0.clear().await
    }
    fn name(&self) -> &str {
        self.0.name()
    }
    async fn watch(
        &self,
    ) -> Result<clipsync::clipboard::ClipboardWatcher, clipsync::clipboard::ClipboardError> {
        self.0.watch().await
    }
}

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
    #[cfg(target_os = "macos")]
    let clipboard = if std::env::var("CLIPSYNC_IMAGE_FAILURE_FIXTURE").as_deref() == Ok("1") {
        clipsync::adapters::ClipboardProviderWrapper::new(Box::new(UncertainImageClipboard(
            clipsync::clipboard::create_provider().await?,
        )))
    } else {
        get_clipboard_provider().await?
    };
    #[cfg(not(target_os = "macos"))]
    let clipboard = get_clipboard_provider().await?;
    let mut engine = SyncEngine::without_discovery(
        Arc::new(config.clone()),
        Arc::new(clipboard),
        history.clone(),
        transport,
    );
    #[cfg(target_os = "macos")]
    if std::env::var("CLIPSYNC_IMAGE_FIXTURE").as_deref() == Ok("1") {
        engine.enable_image_support_for_testing(
            clipsync::transport::image_capability::ImageLimits {
                max_bytes: 1024 * 1024,
                max_width: 8192,
                max_height: 8192,
                max_pixels: 16_777_216,
            },
            384 * 1024 * 1024,
        );
    }
    let engine = Arc::new(engine);
    for sample in [
        "https://example.com/review/clipboard-sync",
        "Review notes\nCheck device recovery after reconnect.\nKeep keyboard navigation clear.",
        "cargo test --locked",
        "teammate@example.com",
        "The latest draft is ready for review. Let me know what you think.",
    ] {
        engine.copy_local(sample.to_owned()).await?;
    }
    #[cfg(target_os = "macos")]
    if std::env::var("CLIPSYNC_IMAGE_FIXTURE").as_deref() == Ok("1") {
        use base64::Engine;
        let png = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAAEUlEQVR4nGP4z8Dwn6HhfwMAEPoD/o3Nc3sAAAAASUVORK5CYII=")?;
        history
            .add_entry(&clipsync::adapters::ClipboardEntry {
                id: uuid::Uuid::new_v4(),
                checksum: clipsync::history::encryption::Encryptor::compute_checksum(&png),
                content: clipsync::adapters::ClipboardData::Image(png),
                timestamp: chrono::Utc::now() + chrono::Duration::seconds(2),
                source: config.node_id,
            })
            .await?;
        if std::env::var("CLIPSYNC_IMAGE_HISTORY_FAILURE_FIXTURE").as_deref() == Ok("1") {
            // Inject a real persistence failure after seeding; reads continue to work.
            rusqlite::Connection::open(&config.clipboard.history_db)?.execute_batch(
                "CREATE TRIGGER fixture_reject_image BEFORE INSERT ON clipboard_history WHEN NEW.content_type = 'image/png' BEGIN SELECT RAISE(FAIL, 'injected history failure'); END;",
            )?;
        }
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
