#![deny(warnings, clippy::all)]
//! Per-user daemon control. The socket directory and peer credentials protect both ends.
//!
//! Protocol version 1 with additive D04 commands and D03 enrollment IPC.
//! One request is read per connection. The handler is dropped if that client
//! disconnects or the 3s deadline expires, so a stalled peer cannot keep the
//! accept loop busy past the bound. Same-user processes remain inside the
//! trust boundary.
//!
//! **Pause / transport limit (honest):** pause clears not-yet-sent `pending` /
//! `queued_sessions` only. Frames already handed to the OS clipboard or peer
//! transport cannot be recalled; there is no transport cancellation API here.
//! Dropping an in-flight control call stops at the current `.await`. Clipboard
//! or history writes that already completed are not rolled back.

mod diagnostics;
pub mod error_codes;
mod history_view;
mod schema;
mod target;

pub use error_codes::{
    engine_features, FEATURE_DIAGNOSTICS, FEATURE_ENROLLMENT, FEATURE_HISTORY_COPY,
    FEATURE_HISTORY_READ, FEATURE_PAUSE, FEATURE_STATUS_V2,
};
pub use schema::{
    Command, ControlError, ControlSnapshot, DaemonStatus, DiagnosticError, DiagnosticsReport,
    EnrollmentCandidateView, EnrollmentOutcomeView, EnrollmentSessionView, EnrollmentSnapshot,
    HistoryEntryDetail, HistoryEntryView, HistoryPage, Request, Response, TrustedDeviceView,
    HISTORY_PAGE_FRAME_MIN, HISTORY_PAGE_MAX, HISTORY_PREVIEW_CHARS, HISTORY_QUERY_MAX_BYTES,
    PROTOCOL_VERSION,
};
pub use target::{EnrollmentTarget, Target};

use crate::config::Config;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::{
    collections::VecDeque,
    fs,
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{unix::OwnedReadHalf, UnixListener, UnixStream},
    time::timeout,
};

const SERVER_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const CLIENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(4);

/// Worst-case JSON string encoding per UTF-8 byte (`\u00XX`).
const JSON_UTF8_BYTE_EXPANSION: usize = 6;
/// Copy request fields outside the string payload (version, keys, braces).
const COPY_REQUEST_FIXED_OVERHEAD: usize = 128;
/// Status/peers/sync responses and non-copy requests.
const RESPONSE_FRAME_HEADROOM: usize = 512;
const MIN_CONTROL_FRAME: usize = 8192;

pub fn max_frame_bytes(config: &Config) -> usize {
    let max_clipboard = config.clipboard.max_size;
    let copy_request = COPY_REQUEST_FIXED_OVERHEAD
        .saturating_add(max_clipboard.saturating_mul(JSON_UTF8_BYTE_EXPANSION));
    copy_request
        .max(RESPONSE_FRAME_HEADROOM)
        .max(MIN_CONTROL_FRAME)
        // History list/search must fit 20 worst-case previews even when
        // clipboard.max_size is configured small.
        .max(HISTORY_PAGE_FRAME_MIN)
}

pub fn socket_path(config: &Config) -> PathBuf {
    use sha2::{Digest, Sha256};
    let identity = format!(
        "{}\n{}",
        config.auth.authorized_keys.display(),
        config.clipboard.history_db.display()
    );
    let hash = hex::encode(Sha256::digest(identity.as_bytes()));
    // User-runtime sockets remain visible when a systemd service uses PrivateTmp.
    #[cfg(target_os = "linux")]
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            let runtime = PathBuf::from(format!("/run/user/{}", unsafe { libc::geteuid() }));
            runtime.is_dir().then_some(runtime)
        })
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(not(target_os = "linux"))]
    let base = std::env::temp_dir();
    base.join(format!("clipsync-{}", unsafe { libc::geteuid() }))
        .join(format!("{}.sock", &hash[..20]))
}

fn private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let created = match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(e.into()),
    };
    // mkdir applies umask. A directory this call just created is pinned to 0700.
    // An existing directory is rejected below rather than widened.
    if created {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
    {
        bail!("Control directory must be owned by this user with mode 0700");
    }
    Ok(())
}

fn ensure_same_user(peer_uid: u32) -> Result<()> {
    if peer_uid != unsafe { libc::geteuid() } {
        bail!("Control peer belongs to another user");
    }
    Ok(())
}

fn check_peer(stream: &UnixStream) -> Result<()> {
    // `peer_cred` failure fails closed: no credential, no dispatch.
    ensure_same_user(stream.peer_cred()?.uid())
}

fn frame_too_large(max_frame: usize) -> String {
    format!("Control frame exceeds {max_frame} bytes")
}

fn apply_snapshot(response: &mut Response, snap: ControlSnapshot) {
    response.peers = snap.peers;
    response.features = Some(snap.features);
    response.instance_id = Some(snap.instance_id);
    response.state_generation = Some(snap.state_generation);
    response.status = Some(snap.status);
}

fn map_handler_error(response: &mut Response, err: anyhow::Error) {
    let message = err.to_string();
    let code = if message.starts_with("Unsupported control protocol") {
        error_codes::UNSUPPORTED_PROTOCOL
    } else if message.starts_with("Clipboard exceeds configured size limit") {
        error_codes::CLIPBOARD_TOO_LARGE
    } else if message.starts_with("Control frame exceeds") {
        error_codes::FRAME_TOO_LARGE
    } else if message.starts_with("Control peer belongs to another user") {
        error_codes::PEER_OTHER_USER
    } else if message.starts_with("Sync paused") {
        error_codes::SYNC_PAUSED
    } else if message.starts_with("History unavailable:") {
        error_codes::HISTORY_UNAVAILABLE
    } else if message.starts_with("History entry not found") {
        error_codes::HISTORY_NOT_FOUND
    } else if message.starts_with("History query too long") {
        error_codes::HISTORY_QUERY_TOO_LONG
    } else if message.starts_with("Resume failed") {
        error_codes::RESUME_FAILED
    } else if message.starts_with("Unsupported control command") {
        error_codes::UNSUPPORTED_COMMAND
    } else if let Some(code) = enrollment_error_code(&message) {
        code
    } else {
        // Non-contract failures: keep legacy error text only; do not invent a code.
        response.error = Some(message);
        return;
    };
    response.fail(code, message);
}

fn enrollment_error_code(message: &str) -> Option<&'static str> {
    const MAP: &[(&str, &str)] = &[
        ("Enrollment closed", error_codes::ENROLLMENT_CLOSED),
        ("Enrollment expired", error_codes::ENROLLMENT_EXPIRED),
        ("Enrollment cancelled", error_codes::ENROLLMENT_CANCELLED),
        (
            "Enrollment rate limited",
            error_codes::ENROLLMENT_RATE_LIMITED,
        ),
        ("Enrollment mismatch", error_codes::ENROLLMENT_MISMATCH),
        ("Enrollment reflection", error_codes::ENROLLMENT_REFLECTION),
        ("Enrollment replay", error_codes::ENROLLMENT_REPLAY),
        ("Enrollment oversize", error_codes::ENROLLMENT_OVERSIZE),
        ("Enrollment busy", error_codes::ENROLLMENT_BUSY),
        (
            "Enrollment session not found",
            error_codes::ENROLLMENT_NOT_FOUND,
        ),
        ("Enrollment incomplete", error_codes::ENROLLMENT_INCOMPLETE),
        (
            "Enrollment unauthorized",
            error_codes::ENROLLMENT_UNAUTHORIZED,
        ),
    ];
    MAP.iter()
        .find(|(prefix, _)| message.starts_with(prefix))
        .map(|(_, code)| *code)
}

/// Bounded control-operation failures for one [`Server`]. Not a host log.
struct ErrorRing {
    entries: VecDeque<DiagnosticError>,
}

impl ErrorRing {
    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    fn push(&mut self, entry: DiagnosticError) {
        while self.entries.len() >= schema::DIAGNOSTICS_ERROR_MAX {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    fn snapshot(&self) -> Vec<DiagnosticError> {
        self.entries.iter().cloned().collect()
    }
}

pub struct Server {
    listener: UnixListener,
    path: PathBuf,
    _lock: fs::File,
    max_frame: usize,
    max_clipboard: usize,
    recent_errors: Mutex<ErrorRing>,
}

impl Server {
    pub fn bind(path: PathBuf, config: &Config) -> Result<Self> {
        let max_frame = max_frame_bytes(config);
        let max_clipboard = config.clipboard.max_size;
        private_directory(path.parent().context("Socket has no parent")?)?;
        let lock_path = path.with_extension("lock");
        let lock = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)?;
        // fchmod the opened inode so umask or a pre-existing mode cannot leave
        // the lock group-readable. O_NOFOLLOW above refuses a symlink.
        if unsafe { libc::fchmod(lock.as_raw_fd(), 0o600) } != 0 {
            bail!("Control lock must be mode 0600");
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("ClipSync daemon already owns this control socket");
        }
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let listener = UnixListener::bind(&path)?;
        if let Err(err) = prepare_socket(&path) {
            drop(listener);
            let _ = fs::remove_file(&path);
            return Err(err);
        }
        Ok(Self {
            listener,
            path,
            _lock: lock,
            max_frame,
            max_clipboard,
            recent_errors: Mutex::new(ErrorRing::new()),
        })
    }

    pub async fn run(&self, target: &dyn Target) -> Result<()> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            match timeout(
                SERVER_REQUEST_TIMEOUT,
                self.handle_connection(stream, target),
            )
            .await
            {
                // Parser errors can echo request text. Keep the log payload-free;
                // the bounded diagnostic ring carries the safe classification.
                Ok(Err(_)) => tracing::debug!("Control request rejected"),
                Err(_) => {
                    tracing::debug!("Control request timed out");
                    self.remember(error_codes::CONTROL_TIMEOUT);
                }
                Ok(Ok(())) => {}
            }
        }
    }

    async fn handle_connection(&self, mut stream: UnixStream, target: &dyn Target) -> Result<()> {
        if let Err(err) = check_peer(&stream) {
            self.remember_rejected_peer(&err);
            return Err(err);
        }
        let request = self.read_request(&mut stream).await?;
        if request.version != PROTOCOL_VERSION {
            let mut response = Response::base();
            response.fail(
                error_codes::UNSUPPORTED_PROTOCOL,
                "Unsupported control protocol",
            );
            self.remember(error_codes::UNSUPPORTED_PROTOCOL);
            return write_json(&mut stream, &response, self.max_frame).await;
        }
        // The request frame is already consumed. Poll disconnect first so a
        // client that is already gone does not start `dispatch`. EOF or a read
        // error drops the target future at its current await.
        let command = request.command;
        let (mut read_half, mut write_half) = stream.into_split();
        let mut response = Response::base();
        tokio::select! {
            biased;
            _ = peer_gone(&mut read_half) => {
                tracing::debug!("Control client disconnected; in-flight request cancelled");
                return Ok(());
            }
            _ = dispatch(target, command, &mut response, self.max_clipboard) => {}
        }
        self.overlay_diagnostics(&mut response);
        self.note_response_error(&response);
        let written = write_json(&mut write_half, &response, self.max_frame).await;
        if written.is_err() && response.error.is_none() {
            self.remember(error_codes::CONTROL_PROTOCOL);
        }
        written
    }

    async fn read_request(&self, stream: &mut UnixStream) -> Result<Request> {
        match read_json(stream, self.max_frame).await {
            Ok(request) => Ok(request),
            Err(ReadFrameError::TooLarge { max_frame }) => {
                let message = frame_too_large(max_frame);
                let mut response = Response::base();
                response.fail(error_codes::FRAME_TOO_LARGE, message.clone());
                self.remember(error_codes::FRAME_TOO_LARGE);
                write_json(stream, &response, self.max_frame).await?;
                bail!("{message}");
            }
            Err(ReadFrameError::Failed(err)) => {
                self.remember(error_codes::CONTROL_PROTOCOL);
                Err(err)
            }
        }
    }

    /// Classifies the credential error. The text is not stored.
    fn remember_rejected_peer(&self, err: &anyhow::Error) {
        let code = if err
            .to_string()
            .starts_with("Control peer belongs to another user")
        {
            error_codes::PEER_OTHER_USER
        } else {
            error_codes::CONTROL_FAILED
        };
        self.remember(code);
    }

    fn note_response_error(&self, response: &Response) {
        if response.error.is_some() {
            self.remember(
                response
                    .error_code
                    .as_deref()
                    .unwrap_or(error_codes::CONTROL_FAILED),
            );
        }
    }

    fn overlay_diagnostics(&self, response: &mut Response) {
        if let Some(report) = response.diagnostics.as_mut() {
            diagnostics::overlay_recent_errors(report, self.recent_errors_snapshot());
        }
    }

    fn remember(&self, code: &str) {
        let entry = diagnostics::diagnostic_error(code);
        self.recent_errors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(entry);
    }

    fn recent_errors_snapshot(&self) -> Vec<DiagnosticError> {
        self.recent_errors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot()
    }
}

fn prepare_socket(path: &Path) -> Result<()> {
    // chmod the filesystem path. fchmod on the listen fd returns EINVAL on
    // macOS and does not change the path mode on Linux (it stays 0755). The
    // parent is already 0700, so another uid cannot traverse during this
    // window. Reject a symlink or a mode that still grants group/other access.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink()
        || !meta.file_type().is_socket()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
    {
        bail!("Control socket must be owned by this user with mode 0600");
    }
    Ok(())
}

async fn dispatch(
    target: &dyn Target,
    command: Command,
    response: &mut Response,
    max_clipboard: usize,
) {
    match command {
        Command::Sync => match target.sync().await {
            Ok(n) => response.queued = Some(n),
            Err(e) => map_handler_error(response, e),
        },
        Command::Copy { text } => {
            if text.len() > max_clipboard {
                response.fail(
                    error_codes::CLIPBOARD_TOO_LARGE,
                    "Clipboard exceeds configured size limit",
                );
            } else {
                let paused = target.is_paused().await;
                match target.copy_text(text).await {
                    Ok(n) => {
                        response.queued = Some(n);
                        // Always present so clients distinguish local-only vs force-sync.
                        response.pause_applied = Some(paused);
                        if let Ok(snap) = target.control_snapshot().await {
                            response.instance_id = Some(snap.instance_id);
                            response.state_generation = Some(snap.state_generation);
                            response.status = Some(snap.status);
                        }
                    }
                    Err(e) => map_handler_error(response, e),
                }
            }
        }
        Command::Status => match target.control_snapshot().await {
            Ok(snap) => apply_snapshot(response, snap),
            Err(e) => {
                response.peers = target.peers().await;
                map_handler_error(response, e);
            }
        },
        Command::Peers => {
            response.peers = target.peers().await;
        }
        Command::SetPause { paused } => match target.set_pause(paused).await {
            Ok(snap) => {
                response.pause_applied = Some(snap.status.paused);
                apply_snapshot(response, snap);
            }
            Err(e) => map_handler_error(response, e),
        },
        Command::HistoryList { limit } => match target.history_list(limit).await {
            Ok(page) => {
                response.history = Some(page);
                if let Ok(snap) = target.control_snapshot().await {
                    response.instance_id = Some(snap.instance_id);
                    response.state_generation = Some(snap.state_generation);
                }
            }
            Err(e) => map_handler_error(response, e),
        },
        Command::HistorySearch { query, limit } => {
            match target.history_search(query, limit).await {
                Ok(page) => {
                    response.history = Some(page);
                    if let Ok(snap) = target.control_snapshot().await {
                        response.instance_id = Some(snap.instance_id);
                        response.state_generation = Some(snap.state_generation);
                    }
                }
                Err(e) => map_handler_error(response, e),
            }
        }
        Command::HistoryGet { id } => match target.history_get(id).await {
            Ok(detail) => {
                response.history_entry = Some(detail);
                // Same tickets as List so controllers can reject stale completions.
                if let Ok(snap) = target.control_snapshot().await {
                    response.instance_id = Some(snap.instance_id);
                    response.state_generation = Some(snap.state_generation);
                }
            }
            Err(e) => map_handler_error(response, e),
        },
        Command::HistoryCopy { id } => match target.history_copy(id).await {
            Ok((queued, paused)) => {
                response.queued = Some(queued);
                response.pause_applied = Some(paused);
                if let Ok(snap) = target.control_snapshot().await {
                    response.instance_id = Some(snap.instance_id);
                    response.state_generation = Some(snap.state_generation);
                    response.status = Some(snap.status);
                }
            }
            Err(e) => map_handler_error(response, e),
        },
        Command::DiagnosticsExport => match target.diagnostics_export().await {
            Ok(report) => response.diagnostics = Some(report),
            Err(e) => map_handler_error(response, e),
        },
        Command::EnrollmentOpen
        | Command::EnrollmentDiscover
        | Command::EnrollmentDial { .. }
        | Command::EnrollmentInspect { .. }
        | Command::EnrollmentConfirm { .. }
        | Command::EnrollmentCancel { .. }
        | Command::EnrollmentRevoke { .. }
        | Command::TrustedDevices => {
            dispatch_enrollment(target, command, response).await;
        }
    }
}

async fn dispatch_enrollment(target: &dyn Target, command: Command, response: &mut Response) {
    let Some(manager) = target.enrollment_manager() else {
        response.fail(
            error_codes::UNSUPPORTED_COMMAND,
            "Unsupported control command",
        );
        return;
    };
    let result = match command {
        Command::EnrollmentOpen => manager.open().await.map(|snap| {
            response.enrollment = Some(snap);
        }),
        Command::EnrollmentDiscover => manager.discover().await.map(|snap| {
            response.enrollment = Some(snap);
        }),
        Command::EnrollmentDial {
            address,
            expected_fingerprint,
            expected_node_id,
        } => manager
            .dial(&address, expected_fingerprint, expected_node_id)
            .await
            .map(|snap| {
                response.enrollment = Some(snap);
            }),
        Command::EnrollmentInspect { session_id } => {
            manager.inspect(session_id).await.map(|snap| {
                response.enrollment = Some(snap);
            })
        }
        Command::EnrollmentConfirm { session_id } => {
            manager.confirm(session_id).await.map(|snap| {
                response.enrollment = Some(snap);
            })
        }
        Command::EnrollmentCancel { session_id } => manager.cancel(session_id).await.map(|snap| {
            response.enrollment = Some(snap);
        }),
        Command::EnrollmentRevoke { fingerprint } => {
            manager.revoke(&fingerprint).await.map(|devices| {
                response.trusted_devices = Some(devices);
            })
        }
        Command::TrustedDevices => manager.trusted_devices().await.map(|devices| {
            response.trusted_devices = Some(devices);
        }),
        _ => unreachable!("enrollment dispatch filter"),
    };
    if let Err(e) = result {
        map_handler_error(response, e);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

enum ReadFrameError {
    TooLarge { max_frame: usize },
    Failed(anyhow::Error),
}

fn read_frame_error(err: ReadFrameError) -> anyhow::Error {
    match err {
        ReadFrameError::TooLarge { max_frame } => anyhow::anyhow!("{}", frame_too_large(max_frame)),
        ReadFrameError::Failed(err) => err,
    }
}

async fn read_json<T, R>(stream: &mut R, max_frame: usize) -> std::result::Result<T, ReadFrameError>
where
    T: serde::de::DeserializeOwned,
    R: AsyncRead + Unpin,
{
    let length = match stream.read_u32().await {
        Ok(length) => length as usize,
        Err(err) => return Err(ReadFrameError::Failed(err.into())),
    };
    if length > max_frame {
        return Err(ReadFrameError::TooLarge { max_frame });
    }
    let mut data = vec![0; length];
    if let Err(err) = stream.read_exact(&mut data).await {
        return Err(ReadFrameError::Failed(err.into()));
    }
    match serde_json::from_slice(&data) {
        Ok(value) => Ok(value),
        Err(err) => Err(ReadFrameError::Failed(err.into())),
    }
}

async fn write_json<T, W>(stream: &mut W, value: &T, max_frame: usize) -> Result<()>
where
    T: Serialize,
    W: AsyncWrite + Unpin,
{
    let data = serde_json::to_vec(value)?;
    let Ok(len) = u32::try_from(data.len()) else {
        bail!("{}", frame_too_large(max_frame));
    };
    if data.len() > max_frame {
        bail!("{}", frame_too_large(max_frame));
    }
    stream.write_u32(len).await?;
    stream.write_all(&data).await?;
    Ok(())
}

/// EOF or a reset after the request frame means the client is gone.
/// Extra bytes are discarded: a connection carries one request.
async fn peer_gone(read: &mut OwnedReadHalf) {
    let mut buf = [0u8; 64];
    loop {
        match read.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

fn daemon_unreachable_message() -> String {
    #[cfg(target_os = "linux")]
    {
        "ClipSync daemon is not reachable. Start it with `clipsync start --foreground` using the same `--config` path.".into()
    }
    #[cfg(not(target_os = "linux"))]
    {
        "ClipSync daemon is not reachable".into()
    }
}

/// Legacy helper: returns Ok(Response) or anyhow error from `error` text only.
/// Prefer [`request_result`] when callers need stable `error_code`.
pub async fn request(path: &Path, command: Command, max_frame: usize) -> Result<Response> {
    match request_result(path, command, max_frame).await {
        Ok(response) => Ok(response),
        Err(ControlError { message, .. }) => bail!("{message}"),
    }
}

/// Raw response including failed envelopes (does not convert `error` into Err).
pub async fn request_raw(path: &Path, command: Command, max_frame: usize) -> Result<Response> {
    timeout(CLIENT_REQUEST_TIMEOUT, async {
        let mut stream = UnixStream::connect(path)
            .await
            .context(daemon_unreachable_message())?;
        check_peer(&stream)?;
        write_json(
            &mut stream,
            &Request {
                version: PROTOCOL_VERSION,
                command,
            },
            max_frame,
        )
        .await?;
        let response: Response = read_json(&mut stream, max_frame)
            .await
            .map_err(read_frame_error)?;
        if response.version != PROTOCOL_VERSION {
            return Err(ControlError {
                code: Some(error_codes::UNSUPPORTED_PROTOCOL.into()),
                message: "Unsupported daemon control protocol".into(),
            }
            .into());
        }
        Ok(response)
    })
    .await
    .context("Daemon control request timed out")?
}

/// Typed request preserving stable `error_code` via [`ControlError`].
pub async fn request_result(
    path: &Path,
    command: Command,
    max_frame: usize,
) -> std::result::Result<Response, ControlError> {
    let response = request_raw(path, command, max_frame).await.map_err(|e| {
        e.downcast::<ControlError>()
            .unwrap_or_else(|error| ControlError {
                code: None,
                message: error.to_string(),
            })
    })?;
    if let Some(err) = ControlError::from_response(&response) {
        return Err(err);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_frame_fits_worst_case_copy_request() {
        let mut config = Config::default();
        config.clipboard.max_size = 16 * 1024;
        let text = "\u{1}".repeat(config.clipboard.max_size);
        let frame = serde_json::to_vec(&Request {
            version: 1,
            command: Command::Copy { text },
        })
        .unwrap();
        assert!(frame.len() <= max_frame_bytes(&config));
    }

    #[test]
    fn max_frame_reserves_history_page_budget_when_max_size_small() {
        let mut config = Config::default();
        config.clipboard.max_size = 1024;
        assert!(max_frame_bytes(&config) >= HISTORY_PAGE_FRAME_MIN);
        // 20 entries × 240-char previews with heavy JSON escaping must fit.
        let preview = "\u{1}".repeat(HISTORY_PREVIEW_CHARS);
        let entries: Vec<_> = (0..20)
            .map(|i| HistoryEntryView {
                id: uuid::Uuid::from_u128(i as u128),
                timestamp: "2026-10-19T00:00:00Z".into(),
                origin_node: uuid::Uuid::nil(),
                content_type: "text/plain".into(),
                content_chars: HISTORY_PREVIEW_CHARS as u32,
                preview: preview.clone(),
            })
            .collect();
        let page = HistoryPage {
            query: Some("q".into()),
            total_returned: 20,
            truncated: true,
            entries,
        };
        let response = Response {
            history: Some(page),
            ..Response::base()
        };
        let frame = serde_json::to_vec(&response).unwrap();
        assert!(
            frame.len() <= max_frame_bytes(&config),
            "history page frame {} exceeds {}",
            frame.len(),
            max_frame_bytes(&config)
        );
    }

    #[test]
    fn preview_folds_controls_and_bounds_length() {
        let preview = history_view::preview_text(&format!("a\nb\tc{}", "x".repeat(300)));
        assert!(!preview.contains('\n'));
        assert!(!preview.contains('\t'));
        assert_eq!(preview.chars().count(), HISTORY_PREVIEW_CHARS);
    }

    #[test]
    fn response_defaults_deserialize_for_phase0_clients() {
        let json = r#"{"version":1,"pid":1,"peers":[],"queued":null,"error":null}"#;
        let response: Response = serde_json::from_str(json).unwrap();
        assert!(response.features.is_none());
        assert!(response.instance_id.is_none());
        assert!(response.history.is_none());
        assert!(response.error_code.is_none());
        assert!(response.enrollment.is_none());
    }

    #[test]
    fn control_error_preserves_code() {
        let mut response = Response::base();
        response.fail(error_codes::SYNC_PAUSED, "Sync paused");
        let err = ControlError::from_response(&response).unwrap();
        assert_eq!(err.code.as_deref(), Some(error_codes::SYNC_PAUSED));
        assert_eq!(err.message, "Sync paused");
    }

    #[test]
    fn other_uid_is_rejected_and_same_uid_is_allowed() {
        let uid = unsafe { libc::geteuid() };
        assert!(ensure_same_user(uid).is_ok());
        let err = ensure_same_user(uid.wrapping_add(1)).unwrap_err();
        assert!(err.to_string().contains("another user"));
    }
}
