//! Local control IPC bounds: framing, version, malformed input, socket permissions,
//! same-user peer credentials, slow clients, and disconnect cancellation.
//! Sockets live in a private temp directory. No personal clipboard, config, or service.

mod support;

use clipsync::{
    config::Config,
    control::{self, error_codes, Command},
};
use std::{
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use support::Task;
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

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

fn perm(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o777
}

struct Counting {
    peers: AtomicUsize,
    copies: AtomicUsize,
}

impl Counting {
    fn new() -> Self {
        Self {
            peers: AtomicUsize::new(0),
            copies: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl control::Target for Counting {
    async fn peers(&self) -> Vec<clipsync::adapters::Peer> {
        self.peers.fetch_add(1, Ordering::SeqCst);
        Vec::new()
    }

    async fn sync(&self) -> anyhow::Result<usize> {
        Ok(0)
    }

    async fn copy_text(&self, _text: String) -> anyhow::Result<usize> {
        self.copies.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    }
}

struct CancelOnDrop {
    flag: Arc<AtomicBool>,
    armed: bool,
}

impl CancelOnDrop {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(true, Ordering::SeqCst);
        }
    }
}

struct Blocking {
    entered: AtomicBool,
    cancelled: Arc<AtomicBool>,
    let_finish: AtomicBool,
    wake: tokio::sync::Notify,
}

impl Blocking {
    fn new() -> Self {
        Self {
            entered: AtomicBool::new(false),
            cancelled: Arc::new(AtomicBool::new(false)),
            let_finish: AtomicBool::new(false),
            wake: tokio::sync::Notify::new(),
        }
    }
}

#[async_trait::async_trait]
impl control::Target for Blocking {
    async fn peers(&self) -> Vec<clipsync::adapters::Peer> {
        Vec::new()
    }

    async fn sync(&self) -> anyhow::Result<usize> {
        self.entered.store(true, Ordering::SeqCst);
        let guard = CancelOnDrop {
            flag: self.cancelled.clone(),
            armed: true,
        };
        let notified = self.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.let_finish.load(Ordering::SeqCst) {
            notified.await;
        }
        guard.disarm();
        Ok(1)
    }

    async fn copy_text(&self, _text: String) -> anyhow::Result<usize> {
        Ok(0)
    }
}

struct Running<T: control::Target> {
    path: PathBuf,
    max_frame: usize,
    probe: Arc<T>,
    _task: Task<anyhow::Result<()>>,
    _dir: TempDir,
}

impl<T: control::Target + 'static> Running<T> {
    async fn spawn(probe: Arc<T>) -> Self {
        let dir = TempDir::new().unwrap();
        let config = isolated_config(dir.path());
        let path = dir.path().join("private").join("control.sock");
        let max_frame = control::max_frame_bytes(&config);
        let server = control::Server::bind(path.clone(), &config).unwrap();
        let target = probe.clone();
        let task = Task(tokio::spawn(
            async move { server.run(target.as_ref()).await },
        ));
        Self {
            path,
            max_frame,
            probe,
            _task: task,
            _dir: dir,
        }
    }
}

async fn write_raw(stream: &mut UnixStream, body: &[u8]) {
    stream.write_u32(body.len() as u32).await.unwrap();
    stream.write_all(body).await.unwrap();
    stream.flush().await.unwrap();
}

async fn read_response(stream: &mut UnixStream) -> anyhow::Result<control::Response> {
    let len = stream.read_u32().await? as usize;
    let mut buf = vec![0; len];
    stream.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

async fn wait_until(mut ready: impl FnMut() -> bool, limit: Duration, what: &str) {
    let started = Instant::now();
    while !ready() {
        if started.elapsed() > limit {
            panic!("{what} (waited {limit:?})");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn socket_is_private_and_unsafe_paths_are_rejected() {
    let dir = TempDir::new().unwrap();
    let config = isolated_config(dir.path());
    let sock_dir = dir.path().join("sock");
    let path = sock_dir.join("c.sock");
    let server = control::Server::bind(path.clone(), &config).unwrap();
    assert_eq!(perm(&sock_dir), 0o700);
    assert_eq!(perm(&path), 0o600);
    assert_eq!(perm(&sock_dir.join("c.lock")), 0o600);
    let sock = fs::symlink_metadata(&path).unwrap();
    assert!(sock.file_type().is_socket());
    assert!(!sock.file_type().is_symlink());
    assert_eq!(sock.uid(), unsafe { libc::geteuid() });
    assert_eq!(fs::symlink_metadata(&sock_dir).unwrap().uid(), sock.uid());
    let err = control::Server::bind(path.clone(), &config)
        .err()
        .expect("invalid socket must be rejected");
    assert!(err.to_string().contains("already owns"));
    drop(server);
    let _again = control::Server::bind(path, &config).unwrap();

    let open = dir.path().join("open");
    fs::create_dir(&open).unwrap();
    fs::set_permissions(&open, fs::Permissions::from_mode(0o777)).unwrap();
    let err = control::Server::bind(open.join("c.sock"), &config)
        .err()
        .expect("invalid socket must be rejected");
    assert!(err.to_string().contains("0700"));
    assert_eq!(perm(&open), 0o777);

    let link_target = dir.path().join("real");
    fs::create_dir(&link_target).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&link_target, &link).unwrap();
    assert!(control::Server::bind(link.join("c.sock"), &config).is_err());
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());

    let not_dir = dir.path().join("not-a-dir");
    fs::write(&not_dir, b"keep").unwrap();
    assert!(control::Server::bind(not_dir.join("c.sock"), &config).is_err());
    assert_eq!(fs::read(&not_dir).unwrap(), b"keep");

    let safe = dir.path().join("safe");
    fs::create_dir(&safe).unwrap();
    fs::set_permissions(&safe, fs::Permissions::from_mode(0o700)).unwrap();
    let sentinel = safe.join("sentinel");
    fs::write(&sentinel, b"keep").unwrap();
    std::os::unix::fs::symlink(&sentinel, safe.join("c.lock")).unwrap();
    assert!(control::Server::bind(safe.join("c.sock"), &config).is_err());
    assert_eq!(fs::read(&sentinel).unwrap(), b"keep");

    let replaced = dir.path().join("replaced");
    fs::create_dir(&replaced).unwrap();
    fs::set_permissions(&replaced, fs::Permissions::from_mode(0o700)).unwrap();
    let target = replaced.join("target-file");
    fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, replaced.join("c.sock")).unwrap();
    let bound = control::Server::bind(replaced.join("c.sock"), &config).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    assert!(fs::symlink_metadata(replaced.join("c.sock"))
        .unwrap()
        .file_type()
        .is_socket());
    drop(bound);

    let widened = dir.path().join("widened");
    fs::create_dir(&widened).unwrap();
    fs::set_permissions(&widened, fs::Permissions::from_mode(0o700)).unwrap();
    let lock = widened.join("c.lock");
    fs::write(&lock, b"lock").unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o666)).unwrap();
    let _bound = control::Server::bind(widened.join("c.sock"), &config).unwrap();
    assert_eq!(perm(&lock), 0o600);
}

#[tokio::test]
async fn bad_frames_do_not_run_commands_and_status_still_works() {
    let running = Running::spawn(Arc::new(Counting::new())).await;
    let probe = &running.probe;

    for version in [0_u32, 2, 99] {
        let mut stream = UnixStream::connect(&running.path).await.unwrap();
        assert_eq!(stream.peer_cred().unwrap().uid(), unsafe {
            libc::geteuid()
        });
        let body = serde_json::to_vec(&serde_json::json!({
            "version": version,
            "command": "Sync"
        }))
        .unwrap();
        write_raw(&mut stream, &body).await;
        let response = read_response(&mut stream).await.unwrap();
        assert_eq!(response.version, 1);
        assert!(response.pid > 0);
        assert!(response.peers.is_empty());
        assert!(response.history.is_none());
        assert!(response.diagnostics.is_none());
        assert!(response
            .error
            .as_deref()
            .unwrap()
            .starts_with("Unsupported control protocol"));
        assert_eq!(
            response.error_code.as_deref(),
            Some(error_codes::UNSUPPORTED_PROTOCOL)
        );
        assert_eq!(probe.peers.load(Ordering::SeqCst), 0);
    }

    let rejected: [&[u8]; 4] = [
        br#"{"version":1,"command":"Status","extra":true}"#,
        br#"{"version":1,"command":"NotACommand"}"#,
        b"{",
        &[0xff, 0xfe, 0x00],
    ];
    for body in rejected {
        let mut stream = UnixStream::connect(&running.path).await.unwrap();
        write_raw(&mut stream, body).await;
        let err = tokio::time::timeout(Duration::from_secs(2), read_response(&mut stream))
            .await
            .expect("malformed frame hung the server")
            .unwrap_err();
        let _ = err;
        assert_eq!(probe.peers.load(Ordering::SeqCst), 0);
    }

    let mut truncated = UnixStream::connect(&running.path).await.unwrap();
    truncated.write_u32(64).await.unwrap();
    truncated.write_all(b"{").await.unwrap();
    drop(truncated);

    let mut empty = UnixStream::connect(&running.path).await.unwrap();
    empty.write_u32(0).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut empty))
            .await
            .expect("empty frame hung the server")
            .is_err()
    );

    let mut oversize = UnixStream::connect(&running.path).await.unwrap();
    oversize
        .write_u32((running.max_frame + 1) as u32)
        .await
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), read_response(&mut oversize))
        .await
        .expect("oversized length hung the server")
        .unwrap();
    assert_eq!(
        response.error_code.as_deref(),
        Some(error_codes::FRAME_TOO_LARGE)
    );
    assert!(response
        .error
        .as_deref()
        .unwrap()
        .starts_with("Control frame exceeds"));
    assert!(response.history.is_none());
    assert_eq!(probe.peers.load(Ordering::SeqCst), 0);
    assert_eq!(probe.copies.load(Ordering::SeqCst), 0);

    let status = control::request(&running.path, Command::Status, running.max_frame)
        .await
        .unwrap();
    assert!(status.error.is_none());
    assert_eq!(status.version, 1);
    assert_eq!(probe.peers.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn oversize_copy_is_rejected_before_the_handler_runs() {
    let dir = TempDir::new().unwrap();
    let mut config = isolated_config(dir.path());
    config.clipboard.max_size = 4;
    let path = dir.path().join("private").join("control.sock");
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let probe = Arc::new(Counting::new());
    let target = probe.clone();
    let _task = Task(tokio::spawn(
        async move { server.run(target.as_ref()).await },
    ));
    let err = control::request_result(
        &path,
        Command::Copy {
            text: "12345".into(),
        },
        max_frame,
    )
    .await
    .unwrap_err();
    assert_eq!(err.code.as_deref(), Some(error_codes::CLIPBOARD_TOO_LARGE));
    assert!(err
        .message
        .starts_with("Clipboard exceeds configured size limit"));
    assert_eq!(probe.copies.load(Ordering::SeqCst), 0);
    let status = control::request(&path, Command::Status, max_frame)
        .await
        .unwrap();
    assert!(status.error.is_none());
    assert_eq!(probe.peers.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn client_rejects_incompatible_and_oversized_responses() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("fake.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let path_for_server = path.clone();
    let server = tokio::spawn(async move {
        let _ = path_for_server;
        let (mut stream, _) = listener.accept().await.unwrap();
        let len = stream.read_u32().await.unwrap() as usize;
        let mut buf = vec![0; len];
        stream.read_exact(&mut buf).await.unwrap();
        let body = br#"{"version":7,"pid":1,"peers":[],"queued":null,"error":null}"#;
        stream.write_u32(body.len() as u32).await.unwrap();
        stream.write_all(body).await.unwrap();
        std::future::pending::<()>().await
    });
    let err = control::request(&path, Command::Status, 8192)
        .await
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("Unsupported daemon control protocol"));
    server.abort();
    let _ = server.await;
    fs::remove_file(&path).unwrap();

    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let len = stream.read_u32().await.unwrap() as usize;
        let mut buf = vec![0; len];
        stream.read_exact(&mut buf).await.unwrap();
        stream.write_u32(8193).await.unwrap();
        std::future::pending::<()>().await
    });
    let err = control::request(&path, Command::Status, 8192)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Control frame exceeds 8192"));
    server.abort();
    let _ = server.await;
    fs::remove_file(&path).unwrap();

    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let len = stream.read_u32().await.unwrap() as usize;
        let mut buf = vec![0; len];
        stream.read_exact(&mut buf).await.unwrap();
        stream.write_u32(1).await.unwrap();
        stream.write_all(b"{").await.unwrap();
        drop(stream);
    });
    let err = control::request(&path, Command::Status, 8192)
        .await
        .unwrap_err();
    assert!(!err.to_string().contains("daemon running"));
    server.abort();
    let _ = server.await;
    fs::remove_file(&path).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn already_closed_client_does_not_start_the_handler() {
    let probe = Arc::new(Blocking::new());
    let running = Running::spawn(probe.clone()).await;
    let stream = UnixStream::connect(&running.path).await.unwrap();
    let body = serde_json::to_vec(&serde_json::json!({"version": 1, "command": "Sync"})).unwrap();
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(&body);
    let mut written = 0;
    while written < frame.len() {
        match stream.try_write(&frame[written..]) {
            Ok(0) => panic!("socket closed before the frame was queued"),
            Ok(n) => written += n,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                panic!("control frame did not fit in the socket buffer");
            }
            Err(err) => panic!("{err}"),
        }
    }
    drop(stream);
    let started = Instant::now();
    let status = control::request(&running.path, Command::Status, running.max_frame)
        .await
        .unwrap();
    assert!(status.error.is_none());
    assert!(
        !probe.entered.load(Ordering::SeqCst),
        "handler started for a client that was already closed"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn client_disconnect_cancels_in_flight_work() {
    let probe = Arc::new(Blocking::new());
    let running = Running::spawn(probe.clone()).await;
    let mut stream = UnixStream::connect(&running.path).await.unwrap();
    let body = serde_json::to_vec(&serde_json::json!({"version": 1, "command": "Sync"})).unwrap();
    write_raw(&mut stream, &body).await;
    wait_until(
        || probe.entered.load(Ordering::SeqCst),
        Duration::from_secs(2),
        "sync handler did not start",
    )
    .await;
    drop(stream);
    let dropped = Instant::now();
    wait_until(
        || probe.cancelled.load(Ordering::SeqCst),
        Duration::from_millis(1000),
        "disconnect did not cancel in-flight work",
    )
    .await;
    assert!(dropped.elapsed() < Duration::from_secs(2));
    let status = control::request(&running.path, Command::Status, running.max_frame)
        .await
        .unwrap();
    assert!(status.error.is_none());
    assert!(!probe.let_finish.load(Ordering::SeqCst));
}

#[tokio::test]
async fn finished_handler_still_returns_its_response() {
    let probe = Arc::new(Blocking::new());
    let running = Running::spawn(probe.clone()).await;
    let path = running.path.clone();
    let max_frame = running.max_frame;
    let request =
        tokio::spawn(async move { control::request(&path, Command::Sync, max_frame).await });
    wait_until(
        || probe.entered.load(Ordering::SeqCst),
        Duration::from_secs(2),
        "sync handler did not start",
    )
    .await;
    probe.let_finish.store(true, Ordering::SeqCst);
    probe.wake.notify_waiters();
    let response = request.await.unwrap().unwrap();
    assert_eq!(response.queued, Some(1));
    assert!(response.error.is_none());
    assert!(!probe.cancelled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn stalled_header_is_bounded_and_the_next_client_runs() {
    let running = Running::spawn(Arc::new(Counting::new())).await;
    let mut stream = UnixStream::connect(&running.path).await.unwrap();
    let started = Instant::now();
    stream.write_u32(32).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(6), async {
        let mut buf = [0u8; 1];
        stream.read(&mut buf).await
    })
    .await;
    let elapsed = started.elapsed();
    match outcome {
        Ok(Ok(0)) | Ok(Err(_)) => {}
        other => panic!("stalled client was not dropped: {other:?}"),
    }
    assert!(
        elapsed >= Duration::from_millis(2500),
        "closed too soon: {elapsed:?}"
    );
    assert!(
        elapsed <= Duration::from_secs(5),
        "closed too late: {elapsed:?}"
    );
    assert_eq!(running.probe.peers.load(Ordering::SeqCst), 0);
    let status = control::request(&running.path, Command::Status, running.max_frame)
        .await
        .unwrap();
    assert!(status.error.is_none());
    assert_eq!(running.probe.peers.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn hung_handler_is_cancelled_on_the_server_deadline() {
    let probe = Arc::new(Blocking::new());
    let running = Running::spawn(probe.clone()).await;
    let path = running.path.clone();
    let max_frame = running.max_frame;
    let request =
        tokio::spawn(async move { control::request(&path, Command::Sync, max_frame).await });
    wait_until(
        || probe.entered.load(Ordering::SeqCst),
        Duration::from_secs(2),
        "sync handler did not start",
    )
    .await;
    let entered = Instant::now();
    wait_until(
        || probe.cancelled.load(Ordering::SeqCst),
        Duration::from_secs(5),
        "server deadline did not cancel the handler",
    )
    .await;
    let elapsed = entered.elapsed();
    assert!(
        elapsed >= Duration::from_millis(2000),
        "cancelled too soon: {elapsed:?}"
    );
    assert!(
        elapsed <= Duration::from_secs(5),
        "cancelled too late: {elapsed:?}"
    );
    let client = tokio::time::timeout(Duration::from_secs(3), request)
        .await
        .expect("client still blocked after the server dropped the handler")
        .unwrap();
    assert!(client.is_err());
    let status = control::request(&running.path, Command::Status, running.max_frame)
        .await
        .unwrap();
    assert!(status.error.is_none());
}

#[tokio::test]
async fn client_times_out_when_the_peer_never_answers() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("silent.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
        drop(stream);
    });
    let started = Instant::now();
    let err = control::request(&path, Command::Status, 8192)
        .await
        .unwrap_err();
    let elapsed = started.elapsed();
    assert!(
        err.to_string().contains("Daemon control request timed out"),
        "{err}"
    );
    assert!(
        elapsed >= Duration::from_millis(3500),
        "timed out too soon: {elapsed:?}"
    );
    assert!(
        elapsed <= Duration::from_secs(6),
        "timed out too late: {elapsed:?}"
    );
    server.abort();
}
