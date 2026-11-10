#![cfg(feature = "integration-tests")]
mod support;

use base64::Engine;
use clipsync::{
    adapters::{ClipboardProviderWrapper, HistoryManager},
    clipboard::{ClipboardContent, ClipboardError, ClipboardProvider, ClipboardWatcher},
    config::Config,
    sync::SyncEngine,
    transport::{image_capability::ImageLimits, Listener, TransportConfig, TransportManager},
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use support::*;
use tokio::sync::Mutex;

#[derive(Clone)]
struct ImageClipboard {
    content: Arc<Mutex<ClipboardContent>>,
    writes: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
    unknown: Arc<AtomicBool>,
    ambiguous: Arc<AtomicBool>,
    owned: Arc<AtomicBool>,
    foreign: Arc<AtomicBool>,
    auto_enable: bool,
    during_reconcile: Arc<Mutex<Option<ClipboardContent>>>,
}
impl Default for ImageClipboard {
    fn default() -> Self {
        Self {
            content: Arc::new(Mutex::new(ClipboardContent::text(""))),
            writes: Arc::new(AtomicUsize::new(0)),
            fail: Arc::new(AtomicBool::new(false)),
            unknown: Arc::new(AtomicBool::new(false)),
            ambiguous: Arc::new(AtomicBool::new(false)),
            owned: Arc::new(AtomicBool::new(false)),
            foreign: Arc::new(AtomicBool::new(false)),
            auto_enable: false,
            during_reconcile: Arc::new(Mutex::new(None)),
        }
    }
}
#[async_trait::async_trait]
impl ClipboardProvider for ImageClipboard {
    fn enable_image_capture(&self) -> bool {
        self.auto_enable
    }
    async fn get_content(&self) -> Result<ClipboardContent, ClipboardError> {
        if self.ambiguous.load(Ordering::SeqCst) {
            return Err(ClipboardError::Ambiguous);
        }
        Ok(self.content.lock().await.clone())
    }
    async fn set_content(&self, value: &ClipboardContent) -> Result<(), ClipboardError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(ClipboardError::Platform(
                "injected unchanged failure".into(),
            ));
        }
        *self.content.lock().await = value.clone();
        if self.unknown.swap(false, Ordering::SeqCst) {
            self.ambiguous.store(true, Ordering::SeqCst);
            return Err(ClipboardError::WriteOutcomeUnknown(
                "injected concurrent append".into(),
            ));
        }
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn reconcile_unknown_image(
        &self,
        expected: &ClipboardContent,
        observed: &ClipboardContent,
    ) -> Result<clipsync::clipboard::ImageAttribution, ClipboardError> {
        use clipsync::clipboard::ImageAttribution::{Foreign, Owned, Unknown};
        if let Some(replacement) = self.during_reconcile.lock().await.take() {
            *self.content.lock().await = replacement;
        }
        if self.ambiguous.load(Ordering::SeqCst) || self.content.lock().await.data != observed.data
        {
            return Ok(Unknown);
        }
        if self.owned.load(Ordering::SeqCst) && expected.data == observed.data {
            Ok(Owned)
        } else if self.foreign.load(Ordering::SeqCst) {
            Ok(Foreign)
        } else {
            Ok(Unknown)
        }
    }
    async fn clear(&self) -> Result<(), ClipboardError> {
        *self.content.lock().await = ClipboardContent::text("");
        Ok(())
    }
    fn name(&self) -> &str {
        "isolated typed memory clipboard"
    }
    async fn watch(&self) -> Result<ClipboardWatcher, ClipboardError> {
        Err(ClipboardError::WatchError("not used".into()))
    }
}

#[tokio::test]
async fn image_sync_is_enabled_by_default_for_image_clipboards() {
    let a = Identity::new().await;
    let b = Identity::new().await;
    a.trust(&b).await;
    b.trust(&a).await;
    let ca = ImageClipboard {
        auto_enable: true,
        ..Default::default()
    };
    let cb = ImageClipboard {
        auto_enable: true,
        ..Default::default()
    };
    let (ea, ta) = fixture(&a, &ca, None).await;
    let (eb, tb) = fixture(&b, &cb, None).await;
    let la = ea.bind_listener().await.unwrap();
    let lb = eb.bind_listener().await.unwrap();
    let address = lb.local_addr();
    let _a = Task(tokio::spawn({
        let engine = ea.clone();
        async move { engine.run(la).await }
    }));
    let _b = Task(tokio::spawn({
        let engine = eb.clone();
        async move { engine.run(lb).await }
    }));
    ta.connect_peer(&b.peer(address)).await.unwrap();
    eventually(async || tb.is_connected(a.id()).await).await;
    *ca.content.lock().await = ClipboardContent::image(png(), "png");
    eventually(async || cb.content.lock().await.data == png()).await;
    assert_eq!(cb.writes.load(Ordering::SeqCst), 1);
    ea.shutdown().await;
    eb.shutdown().await;
}
fn png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAAEUlEQVR4nGP4z8Dwn6HhfwMAEPoD/o3Nc3sAAAAASUVORK5CYII=").unwrap()
}
async fn fixture(
    id: &Identity,
    clipboard: &ImageClipboard,
    limit: Option<usize>,
) -> (Arc<SyncEngine>, Arc<TransportManager>) {
    let mut config = Config::default();
    config.node_id = id.id();
    config.listen_addr = "127.0.0.1:0".into();
    config.auth.ssh_key = id.dir.path().join("identity");
    config.auth.authorized_keys = id.dir.path().join("authorized_keys");
    config.clipboard.history_db = id.dir.path().join("history.db");
    config.clipboard.history_key = Some(id.dir.path().join("history.key"));
    let transport = Arc::new(TransportManager::with_auth(
        TransportConfig::default(),
        id.auth.clone(),
        id.id(),
    ));
    let history = Arc::new(
        HistoryManager::new_with_key_path(
            &id.dir.path().join("history.db"),
            &id.dir.path().join("history.key"),
        )
        .await
        .unwrap(),
    );
    let mut engine = SyncEngine::without_discovery(
        Arc::new(config),
        Arc::new(ClipboardProviderWrapper::new(Box::new(clipboard.clone()))),
        history,
        transport.clone(),
    );
    if let Some(max_bytes) = limit {
        engine.enable_image_support_for_testing(
            ImageLimits {
                max_bytes,
                max_width: 10,
                max_height: 10,
                max_pixels: 100,
            },
            4096,
        );
    }
    (Arc::new(engine), transport)
}

#[tokio::test]
async fn image_sync_is_capability_gated_and_does_not_echo() {
    let a = Identity::new().await;
    let b = Identity::new().await;
    let legacy = Identity::new().await;
    a.trust(&b).await;
    b.trust(&a).await;
    a.trust(&legacy).await;
    legacy.trust(&a).await;
    let ca = ImageClipboard::default();
    let cb = ImageClipboard::default();
    let cc = ImageClipboard::default();
    let (ea, ta) = fixture(&a, &ca, Some(1024)).await;
    let (eb, tb) = fixture(&b, &cb, Some(1024)).await;
    let (ec, tc) = fixture(&legacy, &cc, None).await;
    let la = ea.bind_listener().await.unwrap();
    let lb = eb.bind_listener().await.unwrap();
    let lc = ec.bind_listener().await.unwrap();
    let (b_addr, c_addr) = (lb.local_addr(), lc.local_addr());
    let _a = Task(tokio::spawn({
        let e = ea.clone();
        async move { e.run(la).await }
    }));
    let _b = Task(tokio::spawn({
        let e = eb.clone();
        async move { e.run(lb).await }
    }));
    let _c = Task(tokio::spawn({
        let e = ec.clone();
        async move { e.run(lc).await }
    }));
    ta.connect_peer(&b.peer(b_addr)).await.unwrap();
    ta.connect_peer(&legacy.peer(c_addr)).await.unwrap();
    eventually(async || tb.is_connected(a.id()).await && tc.is_connected(a.id()).await).await;
    *ca.content.lock().await = ClipboardContent::image(png(), "png");
    eventually(async || cb.content.lock().await.mime_type == "image/png").await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert_eq!(cb.content.lock().await.data, png());
    assert_eq!(cc.writes.load(Ordering::SeqCst), 0);
    assert_eq!(ca.writes.load(Ordering::SeqCst), 0);
    assert_eq!(cb.writes.load(Ordering::SeqCst), 1);
    let entries = eb.history_manager().get_recent_entries(10).await.unwrap();
    assert!(entries.iter().any(
        |e| matches!(&e.content, clipsync::adapters::ClipboardData::Image(data) if *data == png())
    ));
    ea.shutdown().await;
    eb.shutdown().await;
    ec.shutdown().await;
}

#[tokio::test]
async fn malformed_and_oversized_images_do_not_mutate_clipboard_or_history() {
    use clipsync::history::encryption::Encryptor;
    use clipsync::transport::protocol::{
        ClipboardData, ClipboardFormat, Message, MessagePayload, MessageType,
    };
    let node = Identity::new().await;
    let origin = Identity::new().await;
    let clipboard = ImageClipboard::default();
    let (engine, _) = fixture(&node, &clipboard, Some(1024)).await;
    let message = |bytes: Vec<u8>, checksum: String| {
        let mut m = Message::new(
            MessageType::ClipboardData,
            MessagePayload::Clipboard(
                ClipboardData {
                    format: ClipboardFormat::Image {
                        mime_type: "image/png".into(),
                    },
                    data: bytes,
                    compression: None,
                    checksum,
                    metadata: Default::default(),
                }
                .into(),
            ),
        );
        m.source_peer_id = Some(origin.id());
        m.correlation_id = Some(uuid::Uuid::new_v4());
        m.sequence = 1;
        m
    };
    let bad = b"not png".to_vec();
    assert!(engine
        .apply_remote(message(bad.clone(), Encryptor::compute_checksum(&bad)))
        .await
        .is_err());
    assert!(engine
        .apply_remote(message(png(), "incorrect".into()))
        .await
        .is_err());
    let big = vec![1; 1025];
    assert!(engine
        .apply_remote(message(big.clone(), Encryptor::compute_checksum(&big)))
        .await
        .is_err());
    assert_eq!(clipboard.writes.load(Ordering::SeqCst), 0);
    assert_eq!(clipboard.content.lock().await.mime_type, "text/plain");
    assert!(engine
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn image_history_restores_a_retained_entry() {
    use clipsync::history::encryption::Encryptor;
    use clipsync::transport::protocol::{
        ClipboardData, ClipboardFormat, Message, MessagePayload, MessageType,
    };
    let node = Identity::new().await;
    let origin = Identity::new().await;
    let clipboard = ImageClipboard::default();
    let (engine, _) = fixture(&node, &clipboard, Some(1024)).await;
    let bytes = png();
    let mut m = Message::new(
        MessageType::ClipboardData,
        MessagePayload::Clipboard(
            ClipboardData {
                format: ClipboardFormat::Image {
                    mime_type: "image/png".into(),
                },
                checksum: Encryptor::compute_checksum(&bytes),
                data: bytes.clone(),
                compression: None,
                metadata: Default::default(),
            }
            .into(),
        ),
    );
    m.source_peer_id = Some(origin.id());
    m.correlation_id = Some(uuid::Uuid::new_v4());
    m.sequence = chrono::Utc::now().timestamp_millis() as u64 + 1000;
    engine.apply_remote(m).await.unwrap();
    let entry = engine
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap()
        .into_iter()
        .find(|e| matches!(e.content, clipsync::adapters::ClipboardData::Image(_)))
        .unwrap();
    *clipboard.content.lock().await = ClipboardContent::text("replacement");
    assert_eq!(engine.history_copy(entry.id).await.unwrap(), 0);
    assert_eq!(clipboard.content.lock().await.data, bytes);
    assert_eq!(clipboard.writes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn peer_over_image_limit_still_receives_newer_text() {
    let a = Identity::new().await;
    let b = Identity::new().await;
    a.trust(&b).await;
    b.trust(&a).await;
    let ca = ImageClipboard::default();
    let cb = ImageClipboard::default();
    let (ea, ta) = fixture(&a, &ca, Some(1024)).await;
    let (eb, tb) = fixture(&b, &cb, Some(16)).await;
    let la = ea.bind_listener().await.unwrap();
    let lb = eb.bind_listener().await.unwrap();
    let addr = lb.local_addr();
    let _a = Task(tokio::spawn({
        let e = ea.clone();
        async move { e.run(la).await }
    }));
    let _b = Task(tokio::spawn({
        let e = eb.clone();
        async move { e.run(lb).await }
    }));
    ta.connect_peer(&b.peer(addr)).await.unwrap();
    eventually(async || tb.is_connected(a.id()).await).await;
    *ca.content.lock().await = ClipboardContent::image(png(), "png");
    tokio::time::sleep(std::time::Duration::from_millis(650)).await;
    assert_eq!(cb.writes.load(Ordering::SeqCst), 0);
    assert!(ea
        .force_sync()
        .await
        .unwrap_err()
        .to_string()
        .contains("exceeds peer limits"));
    ea.copy_local("newer ordinary text".into()).await.unwrap();
    eventually(async || {
        cb.content.lock().await.as_text().as_deref() == Some("newer ordinary text")
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(cb.writes.load(Ordering::SeqCst), 1);
    ea.shutdown().await;
    eb.shutdown().await;
}

#[tokio::test]
async fn history_failure_after_apply_does_not_reapply_remote_event() {
    use clipsync::history::encryption::Encryptor;
    use clipsync::transport::protocol::{
        ClipboardData, ClipboardFormat, Message, MessagePayload, MessageType,
    };
    let node = Identity::new().await;
    let origin = Identity::new().await;
    let clipboard = ImageClipboard::default();
    let (engine, _) = fixture(&node, &clipboard, Some(1024)).await;
    let generation = engine.runtime().await.state_generation;
    let conn = rusqlite::Connection::open(node.dir.path().join("history.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_test_history BEFORE INSERT ON clipboard_history BEGIN SELECT RAISE(FAIL, 'injected failure'); END;").unwrap();
    let bytes = png();
    let mut m = Message::new(
        MessageType::ClipboardData,
        MessagePayload::Clipboard(
            ClipboardData {
                format: ClipboardFormat::Image {
                    mime_type: "image/png".into(),
                },
                checksum: Encryptor::compute_checksum(&bytes),
                data: bytes.clone(),
                compression: None,
                metadata: Default::default(),
            }
            .into(),
        ),
    );
    m.source_peer_id = Some(origin.id());
    m.correlation_id = Some(uuid::Uuid::new_v4());
    m.sequence = chrono::Utc::now().timestamp_millis() as u64 + 1000;
    let error = engine
        .apply_remote(m.clone())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Clipboard applied; history save failed"));
    assert!(engine.runtime().await.state_generation > generation);
    assert_eq!(clipboard.content.lock().await.data, bytes);
    assert_eq!(clipboard.writes.load(Ordering::SeqCst), 1);
    conn.execute_batch("DROP TRIGGER reject_test_history;")
        .unwrap();
    engine.apply_remote(m).await.unwrap();
    assert_eq!(clipboard.writes.load(Ordering::SeqCst), 1);
    assert!(!engine
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap()
        .iter()
        .any(|e| matches!(e.content, clipsync::adapters::ClipboardData::Image(_))));
}

#[tokio::test]
async fn unchanged_native_write_failure_keeps_event_retryable() {
    use clipsync::history::encryption::Encryptor;
    use clipsync::transport::protocol::{
        ClipboardData, ClipboardFormat, Message, MessagePayload, MessageType,
    };
    let node = Identity::new().await;
    let origin = Identity::new().await;
    let clipboard = ImageClipboard::default();
    let (engine, _) = fixture(&node, &clipboard, Some(1024)).await;
    let bytes = png();
    let mut m = Message::new(
        MessageType::ClipboardData,
        MessagePayload::Clipboard(
            ClipboardData {
                format: ClipboardFormat::Image {
                    mime_type: "image/png".into(),
                },
                checksum: Encryptor::compute_checksum(&bytes),
                data: bytes.clone(),
                compression: None,
                metadata: Default::default(),
            }
            .into(),
        ),
    );
    m.source_peer_id = Some(origin.id());
    m.correlation_id = Some(uuid::Uuid::new_v4());
    m.sequence = chrono::Utc::now().timestamp_millis() as u64 + 1000;
    clipboard.fail.store(true, Ordering::SeqCst);
    assert!(engine.apply_remote(m.clone()).await.is_err());
    assert_eq!(clipboard.writes.load(Ordering::SeqCst), 0);
    assert_eq!(
        clipboard.content.lock().await.as_text().as_deref(),
        Some("")
    );
    clipboard.fail.store(false, Ordering::SeqCst);
    engine.apply_remote(m).await.unwrap();
    assert_eq!(clipboard.writes.load(Ordering::SeqCst), 1);
    assert_eq!(clipboard.content.lock().await.data, bytes);
}

#[tokio::test]
async fn delayed_capability_cannot_resurrect_superseded_image_and_reconnect_resets_it() {
    use clipsync::transport::protocol::{ClipboardFormat, Message, MessagePayload, MessageType};
    let a = Identity::new().await;
    let b = Identity::new().await;
    a.trust(&b).await;
    b.trust(&a).await;
    let ca = ImageClipboard::default();
    let cb = ImageClipboard::default();
    let (ea, ta) = fixture(&a, &ca, Some(1024)).await;
    let (eb, tb) = fixture(&b, &cb, None).await;
    let mut incoming = tb.subscribe().await.unwrap();
    let la = ea.bind_listener().await.unwrap();
    let lb = eb.bind_listener().await.unwrap();
    let addr = lb.local_addr();
    let _a = Task(tokio::spawn({
        let e = ea.clone();
        async move { e.run(la).await }
    }));
    let _b = Task(tokio::spawn({
        let e = eb.clone();
        async move { e.run(lb).await }
    }));
    ta.connect_peer(&b.peer(addr)).await.unwrap();
    eventually(async || tb.is_connected(a.id()).await).await;
    *ca.content.lock().await = ClipboardContent::image(png(), "png");
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    ea.copy_local("superseding text".into()).await.unwrap();
    eventually(async || cb.content.lock().await.as_text().as_deref() == Some("superseding text"))
        .await;
    let limits = ImageLimits {
        max_bytes: 1024,
        max_width: 10,
        max_height: 10,
        max_pixels: 100,
    };
    let cap = Message::new(
        MessageType::Capabilities,
        MessagePayload::Capabilities(limits.capabilities()),
    );
    tb.send_to_peer(a.id(), &cap).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    while let Ok(m) = incoming.try_recv() {
        assert!(
            !matches!(m.payload, MessagePayload::Clipboard(ref data) if matches!(data.format, ClipboardFormat::Image { .. }))
        );
    }
    tb.shutdown().await;
    eventually(async || !ta.is_connected(b.id()).await).await;
    ta.connect_peer(&b.peer(addr)).await.unwrap();
    eventually(async || tb.is_connected(a.id()).await).await;
    while incoming.try_recv().is_ok() {}
    *ca.content.lock().await = ClipboardContent::image(png(), "png");
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    while let Ok(m) = incoming.try_recv() {
        assert!(
            !matches!(m.payload, MessagePayload::Clipboard(ref data) if matches!(data.format, ClipboardFormat::Image { .. }))
        );
    }
    tb.send_to_peer(a.id(), &cap).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Ok(m) = incoming.recv().await {
                if matches!(m.payload, MessagePayload::Clipboard(ref data) if matches!(data.format, ClipboardFormat::Image { .. })) { break; }
            }
        }
    }).await.unwrap();
    ea.shutdown().await;
    eb.shutdown().await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "requires explicitly supplied baseline binary with integration-tests feature and isolated named pasteboard"]
async fn baseline_binary_accepts_capabilities_and_still_syncs_text() {
    use clipsync::clipboard::macos::MacOSClipboard;
    use std::{
        os::unix::process::ExitStatusExt,
        process::{Command, Stdio},
    };
    let baseline = std::env::var("CLIPSYNC_BASELINE_BINARY").expect("set baseline binary path");
    let a = Identity::new().await;
    let b = Identity::new().await;
    a.trust(&b).await;
    b.trust(&a).await;
    let ca = ImageClipboard::default();
    let (ea, ta) = fixture(&a, &ca, Some(1024)).await;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let dir = b.dir.path();
    let config_path = dir.join("config.toml");
    std::fs::write(&config_path, format!("listen_addr = \"127.0.0.1:{port}\"\n[auth]\nssh_key = \"{}\"\nauthorized_keys = \"{}\"\n[clipboard]\nhistory_db = \"{}\"\nhistory_key = \"{}\"\n", dir.join("identity").display(), dir.join("authorized_keys").display(), dir.join("history.db").display(), dir.join("history.key").display())).unwrap();
    let board_name = format!("org.clipsync.test.{}", uuid::Uuid::new_v4());
    std::env::set_var("CLIPSYNC_TEST_PASTEBOARD", &board_name);
    let board = MacOSClipboard::new().unwrap();
    std::env::remove_var("CLIPSYNC_TEST_PASTEBOARD");
    struct BoardCleanup(String);
    #[allow(deprecated)]
    impl Drop for BoardCleanup {
        fn drop(&mut self) {
            use cocoa::{
                base::{id, nil},
                foundation::{NSAutoreleasePool, NSString},
            };
            use objc::{class, msg_send, sel, sel_impl};
            unsafe {
                let pool = NSAutoreleasePool::new(nil);
                let name = NSString::alloc(nil).init_str(&self.0);
                let board: id = msg_send![class!(NSPasteboard), pasteboardWithName: name];
                let _: () = msg_send![board, releaseGlobally];
                let _: () = msg_send![name, release];
                let _: () = msg_send![pool, drain];
            }
        }
    }
    let _board_cleanup = BoardCleanup(board_name.clone());
    board
        .set_content(&ClipboardContent::text("baseline initial"))
        .await
        .unwrap();
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let log = std::fs::File::create(dir.join("baseline.log")).unwrap();
    let mut child = ChildGuard(
        Command::new(baseline)
            .args([
                "--config",
                config_path.to_str().unwrap(),
                "start",
                "--foreground",
            ])
            .env("CLIPSYNC_TEST_PASTEBOARD", &board_name)
            .env("TMPDIR", dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap(),
    );
    let listener = ea.bind_listener().await.unwrap();
    let _a = Task(tokio::spawn({
        let e = ea.clone();
        async move { e.run(listener).await }
    }));
    let addr = format!("127.0.0.1:{port}").parse().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                panic!(
                    "baseline exited {status:?}, signal {:?}: {}",
                    status.signal(),
                    std::fs::read_to_string(dir.join("baseline.log")).unwrap()
                );
            }
            if ta.connect_peer(&b.peer(addr)).await.is_ok() && ta.is_connected(b.id()).await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        ta.is_connected(b.id()).await,
        "baseline closed after capability message"
    );
    ea.copy_local("new peer compatibility text".into())
        .await
        .unwrap();
    eventually(async || {
        board
            .get_content()
            .await
            .ok()
            .and_then(|c| c.as_text())
            .as_deref()
            == Some("new peer compatibility text")
    })
    .await;
    board
        .set_content(&ClipboardContent::text("baseline compatibility reply"))
        .await
        .unwrap();
    eventually(async || {
        ca.content.lock().await.as_text().as_deref() == Some("baseline compatibility reply")
    })
    .await;
    *ca.content.lock().await = ClipboardContent::image(png(), "png");
    tokio::time::sleep(std::time::Duration::from_millis(750)).await;
    assert_eq!(
        board.get_content().await.unwrap().as_text().as_deref(),
        Some("baseline compatibility reply")
    );
    assert!(ta.is_connected(b.id()).await);
    ea.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_write_mixed_snapshot_warns_and_next_local_copy_recovers() {
    use clipsync::history::encryption::Encryptor;
    use clipsync::transport::protocol::{
        ClipboardData, ClipboardFormat, Message, MessagePayload, MessageType,
    };
    use std::io::Write;
    #[derive(Clone)]
    struct Writer(Arc<std::sync::Mutex<Vec<u8>>>);
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let output = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(move || Writer(output.clone()))
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let node = Identity::new().await;
    let origin = Identity::new().await;
    let clipboard = ImageClipboard::default();
    let (engine, _) = fixture(&node, &clipboard, Some(1024)).await;
    let listener = engine.bind_listener().await.unwrap();
    let _task = Task(tokio::spawn({
        let e = engine.clone();
        async move { e.run(listener).await }
    }));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let bytes = png();
    let mut m = Message::new(
        MessageType::ClipboardData,
        MessagePayload::Clipboard(
            ClipboardData {
                format: ClipboardFormat::Image {
                    mime_type: "image/png".into(),
                },
                checksum: Encryptor::compute_checksum(&bytes),
                data: bytes,
                compression: None,
                metadata: Default::default(),
            }
            .into(),
        ),
    );
    m.source_peer_id = Some(origin.id());
    m.correlation_id = Some(uuid::Uuid::new_v4());
    m.sequence = chrono::Utc::now().timestamp_millis() as u64 + 1000;
    clipboard.unknown.store(true, Ordering::SeqCst);
    assert!(engine
        .apply_remote(m.clone())
        .await
        .unwrap_err()
        .to_string()
        .contains("may have changed"));
    assert!(engine.apply_remote(m).await.is_err());
    assert_eq!(clipboard.writes.load(Ordering::SeqCst), 0);
    eventually(async || {
        String::from_utf8_lossy(&logs.lock().unwrap()).contains("ambiguous mixed image copy")
    })
    .await;
    assert!(!engine
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap()
        .iter()
        .any(|e| matches!(e.content, clipsync::adapters::ClipboardData::Image(_))));
    *clipboard.content.lock().await = ClipboardContent::text("subsequent user copy");
    clipboard.ambiguous.store(false, Ordering::SeqCst);
    eventually(async || engine.history_manager().get_recent_entries(10).await.unwrap().iter().any(|e| matches!(&e.content, clipsync::adapters::ClipboardData::Text(text) if text == "subsequent user copy"))).await;
    assert!(String::from_utf8_lossy(&logs.lock().unwrap())
        .contains("resumed after an unambiguous copy"));
    engine.shutdown().await;
}

#[tokio::test]
async fn uncertain_write_reconciles_same_event_after_transient_ambiguity() {
    use clipsync::history::encryption::Encryptor;
    use clipsync::transport::protocol::{
        ClipboardData, ClipboardFormat, Message, MessagePayload, MessageType,
    };
    let node = Identity::new().await;
    let origin = Identity::new().await;
    let clipboard = ImageClipboard::default();
    let (engine, _) = fixture(&node, &clipboard, Some(1024)).await;
    let bytes = png();
    let id = uuid::Uuid::new_v4();
    let mut message = Message::new(
        MessageType::ClipboardData,
        MessagePayload::Clipboard(
            ClipboardData {
                format: ClipboardFormat::Image {
                    mime_type: "image/png".into(),
                },
                checksum: Encryptor::compute_checksum(&bytes),
                data: bytes,
                compression: None,
                metadata: Default::default(),
            }
            .into(),
        ),
    );
    message.source_peer_id = Some(origin.id());
    message.correlation_id = Some(id);
    message.sequence = chrono::Utc::now().timestamp_millis() as u64 + 1000;
    clipboard.unknown.store(true, Ordering::SeqCst);
    assert!(engine.apply_remote(message.clone()).await.is_err());
    assert!(engine.force_sync().await.is_err()); // mixed snapshot stays quarantined
    clipboard.ambiguous.store(false, Ordering::SeqCst);
    assert!(engine.force_sync().await.is_err()); // readable but unattributed is still quarantined
    assert!(!engine
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap()
        .iter()
        .any(|e| matches!(e.content, clipsync::adapters::ClipboardData::Image(_))));
    clipboard.owned.store(true, Ordering::SeqCst);
    assert_eq!(engine.force_sync().await.unwrap(), 0);
    let entries = engine
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap();
    let images: Vec<_> = entries
        .iter()
        .filter(|e| matches!(e.content, clipsync::adapters::ClipboardData::Image(_)))
        .collect();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].id, id);
    assert_eq!(images[0].source, origin.id());
    engine.apply_remote(message).await.unwrap();
    let entries = engine
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e.content, clipsync::adapters::ClipboardData::Image(_)))
            .count(),
        1
    );
    assert_eq!(clipboard.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn same_pixels_from_confirmed_new_owner_are_captured_as_new_copy() {
    use clipsync::history::encryption::Encryptor;
    use clipsync::transport::protocol::{
        ClipboardData, ClipboardFormat, Message, MessagePayload, MessageType,
    };
    let node = Identity::new().await;
    let origin = Identity::new().await;
    let clipboard = ImageClipboard::default();
    let (engine, _) = fixture(&node, &clipboard, Some(1024)).await;
    let listener = engine.bind_listener().await.unwrap();
    let _task = Task(tokio::spawn({
        let e = engine.clone();
        async move { e.run(listener).await }
    }));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let bytes = png();
    let original_id = uuid::Uuid::new_v4();
    let mut message = Message::new(
        MessageType::ClipboardData,
        MessagePayload::Clipboard(
            ClipboardData {
                format: ClipboardFormat::Image {
                    mime_type: "image/png".into(),
                },
                checksum: Encryptor::compute_checksum(&bytes),
                data: bytes,
                compression: None,
                metadata: Default::default(),
            }
            .into(),
        ),
    );
    message.source_peer_id = Some(origin.id());
    message.correlation_id = Some(original_id);
    message.sequence = chrono::Utc::now().timestamp_millis() as u64 + 1000;
    clipboard.unknown.store(true, Ordering::SeqCst);
    assert!(engine.apply_remote(message).await.is_err());
    clipboard.ambiguous.store(false, Ordering::SeqCst);
    clipboard.foreign.store(true, Ordering::SeqCst);
    eventually(async || {
        engine
            .history_manager()
            .get_recent_entries(10)
            .await
            .unwrap()
            .iter()
            .any(|e| {
                matches!(e.content, clipsync::adapters::ClipboardData::Image(_))
                    && e.source == node.id()
                    && e.id != original_id
            })
    })
    .await;
    let entries = engine
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap();
    assert!(!entries.iter().any(|e| e.id == original_id));
    engine.shutdown().await;
}

#[tokio::test]
async fn uncertain_history_restore_reconciles_once_and_pause_does_not_replay() {
    use clipsync::history::encryption::Encryptor;
    use clipsync::transport::protocol::{
        ClipboardData, ClipboardFormat, Message, MessagePayload, MessageType,
    };
    let node = Identity::new().await;
    let origin = Identity::new().await;
    let clipboard = ImageClipboard::default();
    let (engine, _) = fixture(&node, &clipboard, Some(1024)).await;
    let bytes = png();
    let id = uuid::Uuid::new_v4();
    let mut message = Message::new(
        MessageType::ClipboardData,
        MessagePayload::Clipboard(
            ClipboardData {
                format: ClipboardFormat::Image {
                    mime_type: "image/png".into(),
                },
                checksum: Encryptor::compute_checksum(&bytes),
                data: bytes,
                compression: None,
                metadata: Default::default(),
            }
            .into(),
        ),
    );
    message.source_peer_id = Some(origin.id());
    message.correlation_id = Some(id);
    message.sequence = chrono::Utc::now().timestamp_millis() as u64 + 1000;
    engine.apply_remote(message).await.unwrap();
    clipboard.unknown.store(true, Ordering::SeqCst);
    assert!(engine.history_copy(id).await.is_err());
    clipboard.ambiguous.store(false, Ordering::SeqCst);
    clipboard.owned.store(true, Ordering::SeqCst);
    assert_eq!(engine.force_sync().await.unwrap(), 0);
    let entries = engine
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e.content, clipsync::adapters::ClipboardData::Image(_)))
            .count(),
        2
    );
    assert_eq!(
        entries
            .iter()
            .filter(
                |e| matches!(e.content, clipsync::adapters::ClipboardData::Image(_))
                    && e.source == node.id()
            )
            .count(),
        1
    );
    engine.set_pause(true).await.unwrap();
    clipboard.owned.store(false, Ordering::SeqCst);
    clipboard.unknown.store(true, Ordering::SeqCst);
    assert!(engine.history_copy(id).await.is_err());
    clipboard.ambiguous.store(false, Ordering::SeqCst);
    clipboard.owned.store(true, Ordering::SeqCst);
    engine.set_pause(false).await.unwrap();
    assert!(engine.force_sync().await.is_err());
    let after = engine
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap();
    assert_eq!(
        after
            .iter()
            .filter(|e| matches!(e.content, clipsync::adapters::ClipboardData::Image(_)))
            .count(),
        2
    );
}

#[tokio::test]
async fn delayed_reconciliation_history_failure_commits_order_without_replay() {
    use clipsync::history::encryption::Encryptor;
    use clipsync::transport::protocol::{
        ClipboardData, ClipboardFormat, Message, MessagePayload, MessageType,
    };
    let node = Identity::new().await;
    let origin = Identity::new().await;
    let clipboard = ImageClipboard::default();
    let (engine, _) = fixture(&node, &clipboard, Some(1024)).await;
    let bytes = png();
    let id = uuid::Uuid::new_v4();
    let mut message = Message::new(
        MessageType::ClipboardData,
        MessagePayload::Clipboard(
            ClipboardData {
                format: ClipboardFormat::Image {
                    mime_type: "image/png".into(),
                },
                checksum: Encryptor::compute_checksum(&bytes),
                data: bytes,
                compression: None,
                metadata: Default::default(),
            }
            .into(),
        ),
    );
    message.source_peer_id = Some(origin.id());
    message.correlation_id = Some(id);
    message.sequence = chrono::Utc::now().timestamp_millis() as u64 + 1000;
    clipboard.unknown.store(true, Ordering::SeqCst);
    assert!(engine.apply_remote(message.clone()).await.is_err());
    let conn = rusqlite::Connection::open(node.dir.path().join("history.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_delayed_history BEFORE INSERT ON clipboard_history BEGIN SELECT RAISE(FAIL, 'injected failure'); END;").unwrap();
    clipboard.ambiguous.store(false, Ordering::SeqCst);
    clipboard.owned.store(true, Ordering::SeqCst);
    assert!(engine
        .force_sync()
        .await
        .unwrap_err()
        .to_string()
        .contains("Clipboard applied; history save failed"));
    conn.execute_batch("DROP TRIGGER reject_delayed_history;")
        .unwrap();
    engine.apply_remote(message).await.unwrap();
    assert_eq!(clipboard.writes.load(Ordering::SeqCst), 0);
    assert!(!engine
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap()
        .iter()
        .any(|e| e.id == id));
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "requires isolated named macOS pasteboards and serial execution"]
async fn two_native_named_pasteboards_sync_png_and_text_without_echo() {
    use clipsync::adapters::ClipboardData as Stored;
    use clipsync::clipboard::macos::MacOSClipboard;
    struct NamedBoard(String);
    impl NamedBoard {
        fn new() -> Self {
            Self(format!("org.clipsync.test.{}", uuid::Uuid::new_v4()))
        }
        fn handle(&self) -> MacOSClipboard {
            std::env::set_var("CLIPSYNC_TEST_PASTEBOARD", &self.0);
            let board = MacOSClipboard::new().unwrap();
            std::env::remove_var("CLIPSYNC_TEST_PASTEBOARD");
            board
        }
    }
    #[allow(deprecated)]
    impl Drop for NamedBoard {
        fn drop(&mut self) {
            use cocoa::{
                base::{id, nil},
                foundation::{NSAutoreleasePool, NSString},
            };
            use objc::{class, msg_send, sel, sel_impl};
            unsafe {
                let pool = NSAutoreleasePool::new(nil);
                let name = NSString::alloc(nil).init_str(&self.0);
                let board: id = msg_send![class!(NSPasteboard), pasteboardWithName: name];
                let _: () = msg_send![board, releaseGlobally];
                let _: () = msg_send![name, release];
                let _: () = msg_send![pool, drain];
            }
        }
    }
    async fn native_fixture(
        id: &Identity,
        board: MacOSClipboard,
    ) -> (Arc<SyncEngine>, Arc<TransportManager>) {
        let mut config = Config::default();
        config.node_id = id.id();
        config.listen_addr = "127.0.0.1:0".into();
        let transport = Arc::new(TransportManager::with_auth(
            TransportConfig::default(),
            id.auth.clone(),
            id.id(),
        ));
        let history = Arc::new(
            HistoryManager::new_with_key_path(
                &id.dir.path().join("history.db"),
                &id.dir.path().join("history.key"),
            )
            .await
            .unwrap(),
        );
        let engine = SyncEngine::without_discovery(
            Arc::new(config),
            Arc::new(ClipboardProviderWrapper::new(Box::new(board))),
            history,
            transport.clone(),
        );
        (Arc::new(engine), transport)
    }
    let a = Identity::new().await;
    let b = Identity::new().await;
    a.trust(&b).await;
    b.trust(&a).await;
    let board_a = NamedBoard::new();
    let board_b = NamedBoard::new();
    let writer_a = board_a.handle();
    let reader_b = board_b.handle();
    writer_a
        .set_content(&ClipboardContent::text("initial A"))
        .await
        .unwrap();
    reader_b
        .set_content(&ClipboardContent::text("initial B"))
        .await
        .unwrap();
    let (ea, ta) = native_fixture(&a, board_a.handle()).await;
    let (eb, tb) = native_fixture(&b, board_b.handle()).await;
    let la = ea.bind_listener().await.unwrap();
    let lb = eb.bind_listener().await.unwrap();
    let addr = lb.local_addr();
    let task_a = Task(tokio::spawn({
        let e = ea.clone();
        async move { e.run(la).await }
    }));
    let task_b = Task(tokio::spawn({
        let e = eb.clone();
        async move { e.run(lb).await }
    }));
    ta.connect_peer(&b.peer(addr)).await.unwrap();
    eventually(async || tb.is_connected(a.id()).await).await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    writer_a
        .set_content(&ClipboardContent::image(png(), "png"))
        .await
        .unwrap();
    eventually(async || {
        reader_b
            .get_content()
            .await
            .is_ok_and(|c| c.mime_type == "image/png" && c.data == png())
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    let images_a = ea
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| matches!(e.content, Stored::Image(_)))
        .count();
    let images_b = eb
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| matches!(e.content, Stored::Image(_)))
        .count();
    assert_eq!((images_a, images_b), (1, 1));
    reader_b
        .set_content(&ClipboardContent::text("native reply"))
        .await
        .unwrap();
    eventually(async || {
        writer_a
            .get_content()
            .await
            .is_ok_and(|c| c.as_text().as_deref() == Some("native reply"))
    })
    .await;
    // Produce TIFF via AppKit rather than an arbitrary encoder, as macOS sources do.
    let tiff = {
        #[allow(deprecated)]
        unsafe {
            use cocoa::{
                base::{id, nil},
                foundation::{NSAutoreleasePool, NSData},
            };
            use objc::{class, msg_send, sel, sel_impl};
            let pool = NSAutoreleasePool::new(nil);
            let image = png();
            let input =
                NSData::dataWithBytes_length_(nil, image.as_ptr() as *const _, image.len() as u64);
            let rep: id = msg_send![class!(NSBitmapImageRep), imageRepWithData: input];
            assert_ne!(rep, nil);
            let native: id = msg_send![rep, TIFFRepresentation];
            assert_ne!(native, nil);
            let len: usize = msg_send![native, length];
            let bytes: *const u8 = msg_send![native, bytes];
            let out = std::slice::from_raw_parts(bytes, len).to_vec();
            let _: () = msg_send![pool, drain];
            out
        }
    };
    let conversion = clipsync::clipboard::png::from_tiff(
        &tiff,
        ImageLimits {
            max_bytes: 16384,
            max_width: 10,
            max_height: 10,
            max_pixels: 100,
        },
        1048576,
    );
    assert!(
        conversion.is_ok(),
        "native TIFF size {} conversion {:?}",
        tiff.len(),
        conversion.err()
    );
    writer_a
        .set_content(&ClipboardContent::image(tiff, "tiff"))
        .await
        .unwrap();
    eventually(async || {
        reader_b
            .get_content()
            .await
            .is_ok_and(|c| c.mime_type == "image/png")
    })
    .await;
    let received = reader_b.get_content().await.unwrap();
    let decoded = clipsync::clipboard::png::decode(
        &received.data,
        ImageLimits {
            max_bytes: 16384,
            max_width: 10,
            max_height: 10,
            max_pixels: 100,
        },
        1048576,
    )
    .unwrap();
    assert_eq!(decoded.rgba, [255, 0, 0, 255, 0, 128, 255, 128]);
    ea.shutdown().await;
    eb.shutdown().await;
    drop(task_a);
    drop(task_b);
}

#[tokio::test]
async fn foreign_replacement_during_attribution_does_not_publish_stale_image() {
    use clipsync::history::encryption::Encryptor;
    use clipsync::transport::protocol::{
        ClipboardData, ClipboardFormat, Message, MessagePayload, MessageType,
    };
    use image::ImageEncoder;
    let node = Identity::new().await;
    let origin = Identity::new().await;
    let clipboard = ImageClipboard::default();
    let (engine, _) = fixture(&node, &clipboard, Some(1024)).await;
    let listener = engine.bind_listener().await.unwrap();
    let _task = Task(tokio::spawn({
        let e = engine.clone();
        async move { e.run(listener).await }
    }));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let original = png();
    let original_id = uuid::Uuid::new_v4();
    let mut message = Message::new(
        MessageType::ClipboardData,
        MessagePayload::Clipboard(
            ClipboardData {
                format: ClipboardFormat::Image {
                    mime_type: "image/png".into(),
                },
                checksum: Encryptor::compute_checksum(&original),
                data: original.clone(),
                compression: None,
                metadata: Default::default(),
            }
            .into(),
        ),
    );
    message.source_peer_id = Some(origin.id());
    message.correlation_id = Some(original_id);
    message.sequence = chrono::Utc::now().timestamp_millis() as u64 + 1000;
    clipboard.unknown.store(true, Ordering::SeqCst);
    assert!(engine.apply_remote(message).await.is_err());
    let mut b = Vec::new();
    image::codecs::png::PngEncoder::new(&mut b)
        .write_image(&[5, 6, 7, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    clipboard.ambiguous.store(false, Ordering::SeqCst);
    clipboard.foreign.store(true, Ordering::SeqCst);
    *clipboard.during_reconcile.lock().await = Some(ClipboardContent::image(b.clone(), "png"));
    assert!(engine.force_sync().await.is_err());
    eventually(async || engine.history_manager().get_recent_entries(10).await.unwrap().iter().any(|e| matches!(&e.content, clipsync::adapters::ClipboardData::Image(data) if data == &b))).await;
    let entries = engine
        .history_manager()
        .get_recent_entries(10)
        .await
        .unwrap();
    assert!(!entries.iter().any(|e| matches!(&e.content, clipsync::adapters::ClipboardData::Image(data) if data == &original)));
    assert_eq!(clipboard.content.lock().await.data, b);
    engine.shutdown().await;
}

#[tokio::test]
async fn control_history_exposes_image_label_copy_and_partial_outcome_without_bytes() {
    use clipsync::{
        control::{self, Command},
        history::encryption::Encryptor,
        transport::protocol::{
            ClipboardData, ClipboardFormat, Message, MessagePayload, MessageType,
        },
    };
    let node = Identity::new().await;
    let origin = Identity::new().await;
    let clipboard = ImageClipboard::default();
    let (engine, _) = fixture(&node, &clipboard, Some(1024)).await;
    let bytes = png();
    let id = uuid::Uuid::new_v4();
    let mut message = Message::new(
        MessageType::ClipboardData,
        MessagePayload::Clipboard(
            ClipboardData {
                format: ClipboardFormat::Image {
                    mime_type: "image/png".into(),
                },
                checksum: Encryptor::compute_checksum(&bytes),
                data: bytes.clone(),
                compression: None,
                metadata: Default::default(),
            }
            .into(),
        ),
    );
    message.source_peer_id = Some(origin.id());
    message.correlation_id = Some(id);
    message.sequence = chrono::Utc::now().timestamp_millis() as u64 + 1000;
    engine.apply_remote(message).await.unwrap();
    let path = control::socket_path(engine.config_ref());
    let max = control::max_frame_bytes(engine.config_ref());
    let server = control::Server::bind(path.clone(), engine.config_ref()).unwrap();
    let _task = Task(tokio::spawn({
        let e = engine.clone();
        async move { server.run(e.as_ref()).await }
    }));
    let page = control::request(&path, Command::HistoryList { limit: 10 }, max)
        .await
        .unwrap();
    let row = page
        .history
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.id == id)
        .unwrap();
    assert_eq!(row.content_type, "image/png");
    assert_eq!(row.preview, "[Image]");
    let detail = control::request(&path, Command::HistoryGet { id }, max)
        .await
        .unwrap();
    let serialized = serde_json::to_string(&detail).unwrap();
    assert!(!serialized.contains(&base64::engine::general_purpose::STANDARD.encode(&bytes)));
    assert_eq!(detail.history_entry.unwrap().text, "[Image]");
    *clipboard.content.lock().await = ClipboardContent::text("replacement");
    control::request(&path, Command::HistoryCopy { id }, max)
        .await
        .unwrap();
    assert_eq!(clipboard.content.lock().await.data, bytes);
    clipboard.unknown.store(true, Ordering::SeqCst);
    let response = control::request_raw(&path, Command::HistoryCopy { id }, max)
        .await
        .unwrap();
    assert_eq!(
        response.error_code.as_deref(),
        Some(control::error_codes::CLIPBOARD_WRITE_UNKNOWN)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interactive_cli_history_copies_image_via_daemon_in_pty() {
    use clipsync::{
        control,
        history::encryption::Encryptor,
        transport::protocol::{
            ClipboardData, ClipboardFormat, Message, MessagePayload, MessageType,
        },
    };
    let node = Identity::new().await;
    let origin = Identity::new().await;
    let clipboard = ImageClipboard::default();
    let (engine, _) = fixture(&node, &clipboard, Some(1024)).await;
    let bytes = png();
    let id = uuid::Uuid::new_v4();
    let mut message = Message::new(
        MessageType::ClipboardData,
        MessagePayload::Clipboard(
            ClipboardData {
                format: ClipboardFormat::Image {
                    mime_type: "image/png".into(),
                },
                checksum: Encryptor::compute_checksum(&bytes),
                data: bytes.clone(),
                compression: None,
                metadata: Default::default(),
            }
            .into(),
        ),
    );
    message.source_peer_id = Some(origin.id());
    message.correlation_id = Some(id);
    message.sequence = chrono::Utc::now().timestamp_millis() as u64 + 1000;
    message.timestamp = chrono::Utc::now() + chrono::Duration::seconds(2);
    engine.apply_remote(message).await.unwrap();
    *clipboard.content.lock().await = ClipboardContent::text("replacement");
    let config_path = node.dir.path().join("config.toml");
    let mut cli_config = engine.config_ref().clone();
    cli_config.listen_addr = "127.0.0.1:18484".into();
    std::fs::write(&config_path, cli_config.to_toml_string().unwrap()).unwrap();
    let path = control::socket_path(engine.config_ref());
    let server = control::Server::bind(path, engine.config_ref()).unwrap();
    let _task = Task(tokio::spawn({
        let e = engine.clone();
        async move { server.run(e.as_ref()).await }
    }));
    let script = r#"import os, pty, select, subprocess, sys, time
master, slave = pty.openpty()
p = subprocess.Popen([sys.argv[1], '--config', sys.argv[2], 'history', '--interactive'], stdin=slave, stdout=slave, stderr=slave, close_fds=True)
os.close(slave)
data = b''
end = time.monotonic() + 10
while time.monotonic() < end and b'[Image]' not in data:
    if select.select([master], [], [], 0.1)[0]:
        try:
            chunk = os.read(master, 65536)
            if not chunk: break
            data += chunk
        except OSError: break
if b'[Image]' not in data:
    p.kill(); p.wait(); sys.stderr.write(data.decode(errors='replace')); sys.exit(1)
os.write(master, b'\r')
try: status = p.wait(timeout=10)
except subprocess.TimeoutExpired: p.kill(); p.wait(); sys.stderr.write('picker did not exit'); sys.exit(1)
while select.select([master], [], [], 0)[0]:
    try:
        chunk = os.read(master, 65536)
        if not chunk: break
        data += chunk
    except OSError: break
os.close(master)
sys.stdout.write(data.decode(errors='replace'))
sys.exit(status)
"#;
    let output = tokio::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(env!("CARGO_BIN_EXE_clipsync"))
        .arg(&config_path)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("[Image]"),
        "screen={}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(clipboard.content.lock().await.data, bytes);
}
