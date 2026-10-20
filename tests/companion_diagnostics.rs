//! Control-operation error ring. This is not a full host log export.
//! The socket parent is the nested directory `Server::bind` creates (mode 0700).
//! A tempfile root may be 0755 and is not that parent.

use clipsync::{
    config::Config,
    control::{self, error_codes, Command},
};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};
use tracing::instrument::WithSubscriber;

const PATH_SENTINEL: &str = "/tmp/clipsync-sentinel-path/history.key";
const CONTENT_SENTINEL: &str = "SENTINEL_CLIPBOARD_ghp_secret_payload";
const PEER_SENTINEL: &str = "sentinel-peer-name";

struct Injected;

#[async_trait::async_trait]
impl control::Target for Injected {
    async fn peers(&self) -> Vec<clipsync::adapters::Peer> {
        Vec::new()
    }

    async fn sync(&self) -> anyhow::Result<usize> {
        anyhow::bail!(
            "Sync paused; retained {CONTENT_SENTINEL} for {PEER_SENTINEL} at {PATH_SENTINEL}"
        )
    }

    async fn copy_text(&self, _text: String) -> anyhow::Result<usize> {
        Ok(1)
    }

    async fn history_list(&self, _limit: u32) -> anyhow::Result<control::HistoryPage> {
        anyhow::bail!(
            "History unavailable: {PATH_SENTINEL} peer {PEER_SENTINEL} row {CONTENT_SENTINEL}"
        )
    }

    async fn history_search(
        &self,
        query: String,
        _limit: u32,
    ) -> anyhow::Result<control::HistoryPage> {
        anyhow::bail!("indexer failed on {query}")
    }

    async fn diagnostics_export(&self) -> anyhow::Result<control::DiagnosticsReport> {
        Ok(control::DiagnosticsReport {
            generated_at: chrono::Utc::now().to_rfc3339(),
            app_version: clipsync::VERSION.to_string(),
            control_version: control::PROTOCOL_VERSION,
            features: vec!["diagnostics".into()],
            paused: false,
            connected_peer_count: 0,
            history_available: false,
            history_capacity: 20,
            max_clipboard_bytes: 1024,
            service_mode_hint: "unknown".into(),
            config_labels: vec!["history_key=configured".into()],
            recent_errors: vec![control::DiagnosticError {
                at: "leaked-at".into(),
                code: "raw_anyhow".into(),
                message: format!("{PATH_SENTINEL} {CONTENT_SENTINEL} {PEER_SENTINEL}"),
            }],
            redaction: "clipsync-diagnostics-v1".into(),
        })
    }
}

struct Running {
    path: PathBuf,
    max_frame: usize,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    logs: LogBuffer,
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Running {
    async fn shutdown(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

fn isolated_config(dir: &Path) -> Config {
    let mut config = Config::default();
    config.listen_addr = "127.0.0.1:0".into();
    config.auth.ssh_key = dir.join("identity");
    config.auth.authorized_keys = dir.join("authorized_keys");
    config.clipboard.history_db = dir.join("history.db");
    config.clipboard.history_key = Some(dir.join("history.key"));
    config.clipboard.max_size = 1024;
    config
}

fn start(dir: &Path) -> Running {
    let config = isolated_config(dir);
    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let logs = LogBuffer::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let task = tokio::spawn(
        async move {
            let target = Injected;
            server.run(&target).await
        }
        .with_subscriber(subscriber),
    );
    Running {
        path,
        max_frame,
        task,
        logs,
    }
}

fn assert_private_socket_parent(socket: &Path, temp_root: &Path) {
    let parent = socket.parent().unwrap();
    assert_ne!(parent, temp_root);
    let meta = fs::symlink_metadata(parent).unwrap();
    assert!(meta.is_dir());
    assert!(!meta.file_type().is_symlink());
    assert_eq!(meta.mode() & 0o777, 0o700);
}

async fn export(running: &Running) -> control::DiagnosticsReport {
    let response = control::request(&running.path, Command::DiagnosticsExport, running.max_frame)
        .await
        .unwrap();
    response.diagnostics.unwrap()
}

async fn send_frame(path: &Path, body: &[u8]) {
    let mut stream = UnixStream::connect(path).await.unwrap();
    stream
        .write_u32(u32::try_from(body.len()).unwrap())
        .await
        .unwrap();
    stream.write_all(body).await.unwrap();
    let _ = stream.shutdown().await;
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf).await;
}

fn assert_entry(entry: &control::DiagnosticError, code: &str, message: &str) {
    assert_eq!(entry.code, code);
    assert_eq!(entry.message, message);
    let at = chrono::DateTime::parse_from_rfc3339(&entry.at).unwrap();
    assert!(at.timestamp() > 1_700_000_000);
}

fn assert_hidden(report: &control::DiagnosticsReport) {
    let encoded = serde_json::to_string(report).unwrap();
    assert!(!encoded.contains(PATH_SENTINEL), "{encoded}");
    assert!(!encoded.contains(CONTENT_SENTINEL), "{encoded}");
    assert!(!encoded.contains(PEER_SENTINEL), "{encoded}");
    assert!(!encoded.contains('/'), "{encoded}");
    assert!(!encoded.contains('\\'), "{encoded}");
    assert!(!encoded.contains("raw_anyhow"), "{encoded}");
}

#[tokio::test]
async fn export_records_safe_codes_and_hides_sentinels() {
    let dir = TempDir::new().unwrap();
    let running = start(dir.path());
    assert_private_socket_parent(&running.path, dir.path());

    let before = export(&running).await;
    assert!(before.recent_errors.is_empty());
    assert_hidden(&before);

    let status = control::request(&running.path, Command::Status, running.max_frame)
        .await
        .unwrap();
    assert!(status.error.is_none());
    let peers = control::request(&running.path, Command::Peers, running.max_frame)
        .await
        .unwrap();
    assert!(peers.error.is_none());

    let history = control::request_result(
        &running.path,
        Command::HistoryList { limit: 1 },
        running.max_frame,
    )
    .await
    .unwrap_err();
    assert_eq!(
        history.code.as_deref(),
        Some(error_codes::HISTORY_UNAVAILABLE)
    );
    assert!(history.message.contains(PATH_SENTINEL));
    assert!(history.message.contains(CONTENT_SENTINEL));
    assert!(history.message.contains(PEER_SENTINEL));

    let sync = control::request_result(&running.path, Command::Sync, running.max_frame)
        .await
        .unwrap_err();
    assert_eq!(sync.code.as_deref(), Some(error_codes::SYNC_PAUSED));
    assert!(sync.message.contains(PATH_SENTINEL));
    assert!(sync.message.contains(CONTENT_SENTINEL));

    let search = control::request_result(
        &running.path,
        Command::HistorySearch {
            query: format!("{CONTENT_SENTINEL} {PATH_SENTINEL} {PEER_SENTINEL}"),
            limit: 1,
        },
        running.max_frame,
    )
    .await
    .unwrap_err();
    assert!(search.code.is_none());
    assert!(search.message.contains(PATH_SENTINEL));
    assert!(search.message.contains(CONTENT_SENTINEL));

    let rejected = serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "command": CONTENT_SENTINEL,
    }))
    .unwrap();
    send_frame(&running.path, &rejected).await;

    let copied = control::request(
        &running.path,
        Command::Copy {
            text: CONTENT_SENTINEL.into(),
        },
        running.max_frame,
    )
    .await
    .unwrap();
    assert_eq!(copied.queued, Some(1));
    assert!(copied.error.is_none());

    let report = export(&running).await;
    assert_eq!(report.recent_errors.len(), 4);
    assert_entry(
        &report.recent_errors[0],
        error_codes::HISTORY_UNAVAILABLE,
        "History is unavailable.",
    );
    assert_entry(
        &report.recent_errors[1],
        error_codes::SYNC_PAUSED,
        "Sync is paused.",
    );
    assert_entry(
        &report.recent_errors[2],
        error_codes::CONTROL_FAILED,
        "The control operation failed.",
    );
    assert_entry(
        &report.recent_errors[3],
        error_codes::CONTROL_PROTOCOL,
        "The control request was rejected.",
    );
    assert_hidden(&report);
    let logs = String::from_utf8(running.logs.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("Control request rejected"), "{logs}");
    for sentinel in [PATH_SENTINEL, CONTENT_SENTINEL, PEER_SENTINEL] {
        assert!(!logs.contains(sentinel), "{logs}");
    }
    running.shutdown().await;
}

#[tokio::test]
async fn ring_keeps_the_newest_twenty_and_drops_the_oldest() {
    let dir = TempDir::new().unwrap();
    let running = start(dir.path());
    assert_private_socket_parent(&running.path, dir.path());

    let stale = serde_json::to_vec(&serde_json::json!({
        "version": 99,
        "command": "Status",
    }))
    .unwrap();
    send_frame(&running.path, &stale).await;
    let mid = export(&running).await;
    assert_eq!(mid.recent_errors.len(), 1);
    assert_entry(
        &mid.recent_errors[0],
        error_codes::UNSUPPORTED_PROTOCOL,
        "The control protocol version is not supported.",
    );

    for _ in 0..20 {
        let err = control::request_result(
            &running.path,
            Command::HistoryList { limit: 1 },
            running.max_frame,
        )
        .await
        .unwrap_err();
        assert!(err.message.contains(PATH_SENTINEL));
        assert!(err.message.contains(CONTENT_SENTINEL));
    }

    let report = export(&running).await;
    assert_eq!(report.recent_errors.len(), 20);
    assert!(report.recent_errors.iter().all(|entry| {
        entry.code == error_codes::HISTORY_UNAVAILABLE && entry.message == "History is unavailable."
    }));
    assert!(report
        .recent_errors
        .iter()
        .all(|entry| entry.code != error_codes::UNSUPPORTED_PROTOCOL));
    assert_hidden(&report);
    running.shutdown().await;
}

#[tokio::test]
async fn second_server_and_restart_have_independent_empty_rings() {
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    let running_a = start(dir_a.path());
    let running_b = start(dir_b.path());
    assert_private_socket_parent(&running_a.path, dir_a.path());
    assert_private_socket_parent(&running_b.path, dir_b.path());
    assert_ne!(running_a.path, running_b.path);

    let err = control::request_result(
        &running_a.path,
        Command::HistoryList { limit: 1 },
        running_a.max_frame,
    )
    .await
    .unwrap_err();
    assert!(err.message.contains(PATH_SENTINEL));
    let report_a = export(&running_a).await;
    assert_eq!(report_a.recent_errors.len(), 1);
    assert_eq!(
        report_a.recent_errors[0].code,
        error_codes::HISTORY_UNAVAILABLE
    );
    assert_hidden(&report_a);

    let report_b = export(&running_b).await;
    assert!(report_b.recent_errors.is_empty());
    assert_hidden(&report_b);

    let path_a = running_a.path.clone();
    running_a.shutdown().await;
    let restarted = start(dir_a.path());
    assert_eq!(restarted.path, path_a);
    let report_restart = export(&restarted).await;
    assert!(report_restart.recent_errors.is_empty());
    assert_hidden(&report_restart);

    let report_b_again = export(&running_b).await;
    assert!(report_b_again.recent_errors.is_empty());

    restarted.shutdown().await;
    running_b.shutdown().await;
}
