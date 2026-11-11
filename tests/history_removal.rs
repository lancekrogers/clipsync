mod support;
use clipsync::{
    adapters::{ClipboardData, ClipboardProviderWrapper, HistoryManager, Peer},
    clipboard::{ClipboardContent, ClipboardError, ClipboardProvider, ClipboardWatcher},
    config::Config,
    control::{self, Command, EnrollmentTarget, HistoryRemovalView, Target},
    enrollment::EnrollmentManager,
    sync::{HistoryRemoval, SyncEngine},
    transport::{
        protocol::HistoryOperation, websocket::WebSocketConfig, Connection, Listener, Message,
        MessagePayload, MessageType, TransportConfig, TransportManager, WebSocketTransport,
    },
};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use support::{eventually, Identity, Task};
use tokio::sync::Mutex;
use uuid::Uuid;

/// Reads fail with `NoContent` after a clear, like the macOS and X11 backends.
#[derive(Clone, Default)]
struct Pasteboard {
    text: Arc<Mutex<Option<String>>>,
    writes: Arc<AtomicUsize>,
}
impl Pasteboard {
    async fn copy(&self, text: &str) {
        *self.text.lock().await = Some(text.into());
    }
    async fn text(&self) -> Option<String> {
        self.text.lock().await.clone()
    }
    fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }
}
#[async_trait::async_trait]
impl ClipboardProvider for Pasteboard {
    async fn get_content(&self) -> Result<ClipboardContent, ClipboardError> {
        match self.text.lock().await.clone() {
            Some(text) => Ok(ClipboardContent::text(text)),
            None => Err(ClipboardError::NoContent),
        }
    }
    async fn set_content(&self, data: &ClipboardContent) -> Result<(), ClipboardError> {
        *self.text.lock().await = Some(String::from_utf8(data.data.clone()).unwrap());
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn clear(&self) -> Result<(), ClipboardError> {
        *self.text.lock().await = None;
        Ok(())
    }
    fn name(&self) -> &str {
        "isolated emptying pasteboard"
    }
    async fn watch(&self) -> Result<ClipboardWatcher, ClipboardError> {
        Err(ClipboardError::WatchError("not used by poller".into()))
    }
}

struct Node {
    identity: Identity,
    clipboard: Pasteboard,
    config: Config,
    engine: Arc<SyncEngine>,
    transport: Arc<TransportManager>,
}

async fn engine_for(
    identity: &Identity,
    clipboard: Box<dyn ClipboardProvider>,
) -> (Config, Arc<SyncEngine>, Arc<TransportManager>) {
    let dir = identity.dir.path();
    let mut config = Config::default();
    config.node_id = identity.id();
    config.listen_addr = "127.0.0.1:0".into();
    config.auth.ssh_key = dir.join("identity");
    config.auth.authorized_keys = dir.join("authorized_keys");
    config.clipboard.history_db = dir.join("history.db");
    config.clipboard.history_key = Some(dir.join("history.key"));
    config.clipboard.max_size = 64 * 1024;
    let transport = Arc::new(TransportManager::with_auth(
        TransportConfig {
            connect_timeout: Duration::from_secs(2),
            keepalive_interval: Duration::from_millis(100),
            ..Default::default()
        },
        identity.auth.clone(),
        identity.id(),
    ));
    let history = Arc::new(
        HistoryManager::new_with_key_path(&dir.join("history.db"), &dir.join("history.key"))
            .await
            .unwrap(),
    );
    let engine = Arc::new(SyncEngine::without_discovery(
        Arc::new(config.clone()),
        Arc::new(ClipboardProviderWrapper::new(clipboard)),
        history,
        transport.clone(),
    ));
    (config, engine, transport)
}

impl Node {
    async fn new(identity: Identity) -> Self {
        let clipboard = Pasteboard::default();
        let (config, engine, transport) = engine_for(&identity, Box::new(clipboard.clone())).await;
        Self {
            identity,
            clipboard,
            config,
            engine,
            transport,
        }
    }

    async fn run(&self) -> (Task<anyhow::Result<()>>, SocketAddr) {
        let listener = self.engine.bind_listener().await.unwrap();
        let addr = listener.local_addr();
        let engine = self.engine.clone();
        (
            Task(tokio::spawn(async move { engine.run(listener).await })),
            addr,
        )
    }

    fn id(&self) -> Uuid {
        self.identity.id()
    }

    async fn texts(&self) -> Vec<String> {
        self.engine
            .history_manager()
            .get_recent_entries(20)
            .await
            .unwrap()
            .into_iter()
            .map(|entry| match entry.content {
                ClipboardData::Text(text) => text,
                ClipboardData::Image(_) => "[image]".into(),
            })
            .collect()
    }

    async fn entry_id(&self, text: &str) -> Option<Uuid> {
        self.engine
            .history_manager()
            .get_recent_entries(20)
            .await
            .unwrap()
            .into_iter()
            .find(|entry| matches!(&entry.content, ClipboardData::Text(t) if t == text))
            .map(|entry| entry.id)
    }
}

async fn trusted_identities() -> (Identity, Identity) {
    let a = Identity::new().await;
    let b = Identity::new().await;
    a.trust(&b).await;
    b.trust(&a).await;
    (a, b)
}

/// Two running engines over the real TLS transport, both advertising history ops.
async fn connected_pair() -> (Node, Node, Vec<Task<anyhow::Result<()>>>) {
    let (a, b) = trusted_identities().await;
    let (a, b) = (Node::new(a).await, Node::new(b).await);
    let (run_a, _) = a.run().await;
    let (run_b, b_addr) = b.run().await;
    a.transport
        .connect_peer(&b.identity.peer(b_addr))
        .await
        .unwrap();
    eventually(async || {
        a.transport.supports_history_ops(b.id()).await
            && b.transport.supports_history_ops(a.id()).await
    })
    .await;
    (a, b, vec![run_a, run_b])
}

const KEEP: &str = "grocery list for the week";
const SECRET: &str = "wifi passphrase is open sesame";

#[tokio::test]
async fn delete_and_clear_reach_peers_without_being_captured_or_synced() {
    let (a, b, _tasks) = connected_pair().await;
    assert_eq!(a.engine.copy_local(KEEP.into()).await.unwrap(), 1);
    eventually(async || b.clipboard.text().await.as_deref() == Some(KEEP)).await;
    assert_eq!(a.engine.copy_local(SECRET.into()).await.unwrap(), 1);
    eventually(async || b.clipboard.text().await.as_deref() == Some(SECRET)).await;
    let secret = a.entry_id(SECRET).await.unwrap();
    eventually(async || b.entry_id(SECRET).await == Some(secret)).await;

    let (writes_a, writes_b) = (a.clipboard.writes(), b.clipboard.writes());
    let removal = a.engine.history_delete(secret).await.unwrap();
    assert_eq!(
        removal,
        HistoryRemoval {
            removed: 1,
            clipboard_cleared: true,
            peers_queued: 1,
            peers_skipped: 0,
        }
    );
    assert_eq!(a.clipboard.text().await, None);
    assert!(!a.engine.runtime().await.outbound_pending);
    eventually(async || b.entry_id(SECRET).await.is_none()).await;
    eventually(async || b.clipboard.text().await.is_none()).await;
    // Several capture ticks: the cleared clipboard is neither recorded nor synced.
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(a.texts().await, [KEEP]);
    assert_eq!(b.texts().await, [KEEP]);
    assert_eq!(
        (a.clipboard.writes(), b.clipboard.writes()),
        (writes_a, writes_b)
    );

    // Copying the removed text again is a new copy with a new id.
    a.clipboard.copy(SECRET).await;
    eventually(async || b.clipboard.text().await.as_deref() == Some(SECRET)).await;
    let recopied = a.entry_id(SECRET).await.unwrap();
    assert_ne!(recopied, secret);
    eventually(async || b.entry_id(SECRET).await == Some(recopied)).await;

    const BETA: &str = "copied on beta";
    b.clipboard.copy(BETA).await;
    eventually(async || a.clipboard.text().await.as_deref() == Some(BETA)).await;
    let (writes_a, writes_b) = (a.clipboard.writes(), b.clipboard.writes());
    let removal = a.engine.history_clear().await.unwrap();
    assert_eq!(
        removal,
        HistoryRemoval {
            removed: 3,
            clipboard_cleared: true,
            peers_queued: 1,
            peers_skipped: 0,
        }
    );
    eventually(async || b.texts().await.is_empty()).await;
    eventually(async || b.clipboard.text().await.is_none()).await;
    // Beta originated the last copy; the clear must not leave it queued for new sessions.
    eventually(async || !b.engine.runtime().await.outbound_pending).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(a.texts().await.is_empty());
    assert!(b.texts().await.is_empty());
    assert_eq!(a.clipboard.text().await, None);
    assert_eq!(
        (a.clipboard.writes(), b.clipboard.writes()),
        (writes_a, writes_b)
    );

    // The receiving side also captures a re-copy of what it just cleared.
    b.clipboard.copy(BETA).await;
    eventually(async || a.clipboard.text().await.as_deref() == Some(BETA)).await;
    assert_eq!(a.texts().await, [BETA]);
}

#[tokio::test]
async fn only_clipboards_still_holding_the_entry_are_cleared_and_pause_does_not_block() {
    let (a, b, _tasks) = connected_pair().await;
    assert_eq!(a.engine.copy_local(SECRET.into()).await.unwrap(), 1);
    eventually(async || b.clipboard.text().await.as_deref() == Some(SECRET)).await;
    let secret = a.entry_id(SECRET).await.unwrap();
    eventually(async || b.entry_id(SECRET).await == Some(secret)).await;

    a.engine.set_pause(true).await.unwrap();
    b.engine.set_pause(true).await.unwrap();
    a.clipboard.copy("copied on alpha while paused").await;
    let removal = a.engine.history_delete(secret).await.unwrap();
    assert_eq!(
        removal,
        HistoryRemoval {
            removed: 1,
            clipboard_cleared: false,
            peers_queued: 1,
            peers_skipped: 0,
        }
    );
    assert_eq!(
        a.clipboard.text().await.as_deref(),
        Some("copied on alpha while paused")
    );
    eventually(async || b.entry_id(SECRET).await.is_none()).await;
    eventually(async || b.clipboard.text().await.is_none()).await;

    // Resuming over an empty clipboard baselines it instead of failing.
    b.engine.set_pause(false).await.unwrap();
    a.engine.set_pause(false).await.unwrap();
    b.clipboard.copy(SECRET).await;
    eventually(async || a.clipboard.text().await.as_deref() == Some(SECRET)).await;
    assert!(a.entry_id(SECRET).await.is_some_and(|id| id != secret));
}

#[tokio::test]
async fn peers_without_the_capability_are_skipped_and_stay_connected() {
    let (a, b) = trusted_identities().await;
    let legacy = Identity::new().await;
    a.trust(&legacy).await;
    legacy.trust(&a).await;
    let (a, b) = (Node::new(a).await, Node::new(b).await);
    let (_run_a, a_addr) = a.run().await;
    let (_run_b, b_addr) = b.run().await;
    a.transport
        .connect_peer(&b.identity.peer(b_addr))
        .await
        .unwrap();
    eventually(async || a.transport.supports_history_ops(b.id()).await).await;
    assert_eq!(a.engine.copy_local(SECRET.into()).await.unwrap(), 1);
    let secret = a.entry_id(SECRET).await.unwrap();
    eventually(async || b.entry_id(SECRET).await == Some(secret)).await;
    let removal = a.engine.history_delete(secret).await.unwrap();
    assert_eq!((removal.peers_queued, removal.peers_skipped), (1, 0));

    // A peer that never advertises capabilities, as builds before this message did.
    let mut old = WebSocketTransport::connect_to_peer(
        &a.identity.peer(a_addr),
        legacy.auth.clone(),
        WebSocketConfig {
            connect_timeout: Duration::from_secs(2),
            keepalive_interval: Duration::from_millis(100),
            ..Default::default()
        },
        legacy.id(),
    )
    .await
    .unwrap();
    let frames: Arc<Mutex<Vec<Message>>> = Arc::default();
    let received = frames.clone();
    let _reader = Task(tokio::spawn(async move {
        while let Ok(message) = old.receive().await {
            received.lock().await.push(message);
        }
    }));
    eventually(async || a.transport.is_connected(legacy.id()).await).await;
    let clipboard_frame = |text: &'static str| {
        let frames = frames.clone();
        async move {
            frames.lock().await.iter().any(|m| {
                matches!(&m.payload, MessagePayload::Clipboard(data) if data.data == text.as_bytes())
            })
        }
    };

    // Capture ticks re-offer the last copy to new sessions; a deleted one must not be.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(a.engine.copy_local(KEEP.into()).await.unwrap(), 2);
    eventually(async || clipboard_frame(KEEP).await).await;
    assert!(!clipboard_frame(SECRET).await);
    let keep = a.entry_id(KEEP).await.unwrap();
    eventually(async || b.entry_id(KEEP).await == Some(keep)).await;
    let removal = a.engine.history_delete(keep).await.unwrap();
    assert_eq!((removal.peers_queued, removal.peers_skipped), (1, 1));
    eventually(async || b.entry_id(KEEP).await.is_none()).await;

    // Same session, later frame: the legacy peer is still served and never saw a history op.
    const AFTER: &str = "after the legacy-peer delete";
    assert_eq!(a.engine.copy_local(AFTER.into()).await.unwrap(), 2);
    eventually(async || clipboard_frame(AFTER).await).await;
    assert!(!frames
        .lock()
        .await
        .iter()
        .any(|m| m.message_type == MessageType::HistoryOperation));
    assert!(a.transport.is_connected(legacy.id()).await);
    assert!(!a.transport.supports_history_ops(legacy.id()).await);
}

#[tokio::test]
async fn a_clipboard_that_reads_back_empty_after_clear_is_not_captured() {
    let identity = Identity::new().await;
    let clipboard = support::Clipboard::default();
    let (_, engine, _) = engine_for(&identity, Box::new(clipboard.clone())).await;
    let listener = engine.bind_listener().await.unwrap();
    let runner = engine.clone();
    let _run = Task(tokio::spawn(async move { runner.run(listener).await }));
    // Let run() take its startup baseline before the copy.
    tokio::time::sleep(Duration::from_millis(50)).await;
    engine.copy_local(SECRET.into()).await.unwrap();
    let history = engine.history_manager();
    let secret = history.get_recent_entries(1).await.unwrap()[0].id;
    assert!(
        engine
            .history_delete(secret)
            .await
            .unwrap()
            .clipboard_cleared
    );
    assert_eq!(*clipboard.text.lock().await, "");
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(history.get_recent_entries(20).await.unwrap().is_empty());
}

#[tokio::test]
async fn remote_operations_require_an_authenticated_foreign_origin() {
    let node = Node::new(Identity::new().await).await;
    node.engine.copy_local(SECRET.into()).await.unwrap();
    let secret = node.entry_id(SECRET).await.unwrap();
    let operation = |origin: Option<Uuid>| {
        let mut message = Message::new(
            MessageType::HistoryOperation,
            MessagePayload::History(HistoryOperation::Delete { id: secret }),
        );
        message.source_peer_id = origin;
        message
    };
    assert!(node.engine.apply_remote(operation(None)).await.is_err());
    assert!(node
        .engine
        .apply_remote(operation(Some(node.id())))
        .await
        .is_err());
    assert_eq!(node.entry_id(SECRET).await, Some(secret));
    node.engine.set_pause(true).await.unwrap();
    node.engine
        .apply_remote(operation(Some(Uuid::new_v4())))
        .await
        .unwrap();
    assert_eq!(node.entry_id(SECRET).await, None);
    assert_eq!(node.clipboard.text().await, None);
}

async fn control_request(config: &Config, command: Command) -> control::Response {
    control::request(
        &control::socket_path(config),
        command,
        control::max_frame_bytes(config),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn control_commands_report_results_through_the_daemon_target() {
    let node = Node::new(Identity::new().await).await;
    let manager = Arc::new(
        EnrollmentManager::new(Arc::new(node.config.clone()), node.identity.auth.clone()).unwrap(),
    );
    let target = EnrollmentTarget::new(node.engine.clone(), manager);
    let server = control::Server::bind(control::socket_path(&node.config), &node.config).unwrap();
    let _server = Task(tokio::spawn(async move { server.run(&target).await }));
    let config = &node.config;

    let status = control_request(config, Command::Status).await;
    let features = status.features.unwrap();
    for feature in [
        control::FEATURE_HISTORY_DELETE,
        control::FEATURE_HISTORY_CLEAR,
    ] {
        assert!(features.iter().any(|f| f == feature), "{features:?}");
    }
    for text in [KEEP, SECRET] {
        control_request(config, Command::Copy { text: text.into() }).await;
    }
    let keep = node.entry_id(KEEP).await.unwrap();
    let secret = node.entry_id(SECRET).await.unwrap();
    let generation = control_request(config, Command::Status)
        .await
        .state_generation
        .unwrap();

    let deleted = control_request(config, Command::HistoryDelete { id: keep }).await;
    assert_eq!(
        deleted.history_removal,
        Some(HistoryRemovalView {
            removed: 1,
            clipboard_cleared: false,
            peers_notified: 0,
            peers_skipped: 0,
        })
    );
    assert!(deleted.state_generation.unwrap() > generation);
    assert_eq!(node.texts().await, [SECRET]);
    assert_eq!(node.clipboard.text().await.as_deref(), Some(SECRET));

    let again = control_request(config, Command::HistoryDelete { id: keep }).await;
    assert_eq!(again.history_removal.unwrap().removed, 0);

    let held = control_request(config, Command::HistoryDelete { id: secret }).await;
    let held = held.history_removal.unwrap();
    assert!(held.clipboard_cleared);
    assert_eq!(node.clipboard.text().await, None);

    control_request(config, Command::Copy { text: KEEP.into() }).await;
    let cleared = control_request(config, Command::HistoryClear).await;
    assert_eq!(
        cleared.history_removal,
        Some(HistoryRemovalView {
            removed: 1,
            clipboard_cleared: true,
            peers_notified: 0,
            peers_skipped: 0,
        })
    );
    assert!(node.texts().await.is_empty());
}

/// A daemon target from before these commands existed.
struct LegacyTarget;
#[async_trait::async_trait]
impl Target for LegacyTarget {
    async fn peers(&self) -> Vec<Peer> {
        Vec::new()
    }
    async fn sync(&self) -> anyhow::Result<usize> {
        Ok(0)
    }
    async fn copy_text(&self, _text: String) -> anyhow::Result<usize> {
        Ok(0)
    }
}

async fn cli(config_path: &std::path::Path, args: &[&str]) -> std::process::Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_clipsync"))
        .arg("--config")
        .arg(config_path)
        .args(args)
        .output()
        .await
        .unwrap()
}

#[tokio::test]
async fn cli_deletes_and_clears_through_the_daemon_and_checks_features() {
    let node = Node::new(Identity::new().await).await;
    let config_path = node.identity.dir.path().join("config.toml");
    // The CLI validates a daemon config; the socket path ignores the listen port.
    let mut cli_config = node.config.clone();
    cli_config.listen_addr = "127.0.0.1:8484".into();
    std::fs::write(&config_path, toml::to_string(&cli_config).unwrap()).unwrap();
    let server = control::Server::bind(control::socket_path(&node.config), &node.config).unwrap();
    let engine = node.engine.clone();
    let task = Task(tokio::spawn(
        async move { server.run(engine.as_ref()).await },
    ));
    for text in [KEEP, SECRET] {
        control_request(&node.config, Command::Copy { text: text.into() }).await;
    }
    let keep = node.entry_id(KEEP).await.unwrap();

    let listed = cli(&config_path, &["history", "--ids"]).await;
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert!(String::from_utf8_lossy(&listed.stdout).contains(&keep.to_string()));

    let deleted = cli(&config_path, &["history", "delete", &keep.to_string()]).await;
    let stdout = String::from_utf8_lossy(&deleted.stdout);
    assert!(deleted.status.success(), "{stdout}");
    assert!(stdout.contains("Deleted history entry"), "{stdout}");
    assert_eq!(node.texts().await, [SECRET]);

    // stdin is not a terminal: clearing needs an explicit --yes.
    let refused = cli(&config_path, &["history", "clear"]).await;
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--yes"));
    assert_eq!(node.texts().await, [SECRET]);

    let cleared = cli(&config_path, &["history", "clear", "--yes"]).await;
    let stdout = String::from_utf8_lossy(&cleared.stdout);
    assert!(cleared.status.success(), "{stdout}");
    assert!(stdout.contains("Cleared 1 history entries"), "{stdout}");
    assert!(stdout.contains("Cleared the clipboard"), "{stdout}");
    assert!(node.texts().await.is_empty());
    drop(task);

    // A daemon without the feature is refused before the command is sent.
    let server = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match control::Server::bind(control::socket_path(&node.config), &node.config) {
                Ok(server) => break server,
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .unwrap();
    let _legacy = Task(tokio::spawn(async move { server.run(&LegacyTarget).await }));
    let status = control_request(&node.config, Command::Status).await;
    assert!(!status
        .features
        .unwrap_or_default()
        .iter()
        .any(|f| f == control::FEATURE_HISTORY_DELETE));
    let refused = cli(&config_path, &["history", "delete", &keep.to_string()]).await;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("does not support history_delete"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let unsupported = control::request_result(
        &control::socket_path(&node.config),
        Command::HistoryClear,
        control::max_frame_bytes(&node.config),
    )
    .await
    .unwrap_err();
    assert_eq!(unsupported.code.as_deref(), Some("unsupported_command"));
}
