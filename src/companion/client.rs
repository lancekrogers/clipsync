//! A bounded background IPC worker shared by the GUI and TUI.
use super::model::{
    display_text, friendly_device_name, Connection, Domain, Reconciliation, Route, SnapshotOrder,
    Ticket,
};
use crate::{
    config::Config,
    control::{
        self, error_codes, Command, ControlError, DaemonStatus, DiagnosticError, DiagnosticsReport,
        EnrollmentSnapshot, HistoryRemovalView, ImagePreview, PairingCode, Response,
        TrustedDeviceView,
    },
};
use std::{
    collections::{HashMap, VecDeque},
    sync::mpsc::{self, TrySendError},
    time::{Duration, Instant},
};
use uuid::Uuid;

const DIAGNOSTICS_REDACTION: &str = "clipsync-diagnostics-v1";

#[derive(Clone, Debug)]
pub struct HistoryRow {
    pub id: Uuid,
    pub preview: String,
    pub timestamp: String,
    pub source: String,
    pub characters: u32,
    pub content_type: String,
    /// Renderers resolve names and relative times from these at draw time.
    pub origin: Uuid,
    pub copied_at: Option<chrono::DateTime<chrono::Local>>,
}
#[derive(Clone, Debug)]
pub struct DeviceRow {
    pub id: Uuid,
    pub name: String,
    pub address: String,
}
/// Digits typed toward a pairing code. `Debug` hides them.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CodeEntry(String);
impl CodeEntry {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn is_complete(&self) -> bool {
        self.0.len() == PairingCode::DIGITS
    }
}
impl std::fmt::Debug for CodeEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CodeEntry({} digits)", self.0.len())
    }
}

#[derive(Clone, Debug)]
pub struct View {
    pub route: Route,
    pub connection: Connection,
    pub stale: bool,
    pub devices: Vec<DeviceRow>,
    pub node_id: String,
    /// Trusted devices and revoke (either enrollment feature).
    pub enrollment_supported: bool,
    /// Pairing with a code (`enrollment_code`).
    pub pairing_supported: bool,
    pub history_delete_supported: bool,
    pub history_clear_supported: bool,
    pub enrollment: Option<EnrollmentSnapshot>,
    pub trusted_devices: Vec<TrustedDeviceView>,
    pub device_address: String,
    pub pairing_code: CodeEntry,
    pub selected_session: Option<Uuid>,
    pub history: Vec<HistoryRow>,
    pub query: String,
    pub selected: Option<Uuid>,
    pub detail: Option<String>,
    pub image_preview: Option<ImagePreview>,
    pub detail_page: usize,
    pub notice: Option<String>,
    pub error: Option<String>,
    pub error_code: Option<String>,
    pub diagnostics: Option<String>,
    pub action_pending: bool,
    pub history_loading: bool,
    pub detail_loading: bool,
    /// Peer names (TLS fingerprints) seen connected this session, by node id,
    /// so history from a device that dropped off keeps its name.
    pub seen_peers: HashMap<Uuid, String>,
}
impl Default for View {
    fn default() -> Self {
        Self {
            route: Route::Overview,
            connection: Connection::Checking,
            stale: true,
            devices: Vec::new(),
            node_id: String::new(),
            enrollment_supported: false,
            pairing_supported: false,
            history_delete_supported: false,
            history_clear_supported: false,
            enrollment: None,
            trusted_devices: Vec::new(),
            device_address: String::new(),
            pairing_code: CodeEntry::default(),
            selected_session: None,
            history: Vec::new(),
            query: String::new(),
            selected: None,
            detail: None,
            image_preview: None,
            detail_page: 0,
            notice: None,
            error: None,
            error_code: None,
            diagnostics: None,
            action_pending: false,
            history_loading: false,
            detail_loading: false,
            seen_peers: HashMap::new(),
        }
    }
}
impl View {
    /// Keep text layout bounded even when a retained copy is several megabytes.
    pub fn detail_pages(&self) -> usize {
        self.detail
            .as_ref()
            .map_or(1, |text| text.len().div_ceil(8192).max(1))
    }
    pub fn visible_detail(&self) -> Option<&str> {
        let text = self.detail.as_deref()?;
        let page = self.detail_page.min(self.detail_pages() - 1);
        let boundary = |mut at: usize| {
            at = at.min(text.len());
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            at
        };
        Some(&text[boundary(page * 8192)..boundary((page + 1) * 8192)])
    }

    pub fn can_act(&self) -> bool {
        !self.stale
            && !self.action_pending
            && matches!(
                self.connection,
                Connection::Ready | Connection::Connected(_) | Connection::Paused
            )
    }
    pub fn is_local(&self, id: Uuid) -> bool {
        !self.node_id.is_empty() && id.to_string() == self.node_id
    }
    /// The friendliest known name: the authorization comment for the peer's
    /// fingerprint, then its advertised name, then a short identifier.
    pub fn device_name(&self, id: Uuid) -> String {
        if self.is_local(id) {
            return "This computer".into();
        }
        let advertised = || {
            self.enrollment.as_ref().and_then(|snapshot| {
                snapshot
                    .candidates
                    .iter()
                    .find(|candidate| candidate.advertised_node_id == Some(id))
                    .and_then(|candidate| friendly_device_name(&candidate.display_name))
            })
        };
        let peer = self
            .devices
            .iter()
            .find(|device| device.id == id)
            .map(|device| device.name.as_str())
            .or_else(|| self.seen_peers.get(&id).map(String::as_str));
        let Some(peer) = peer else {
            return advertised().unwrap_or_else(|| format!("Device {}", &id.to_string()[..8]));
        };
        if let Some(fingerprint) = peer.strip_prefix("SHA256:") {
            self.trusted_devices
                .iter()
                .find(|device| device.fingerprint == peer)
                .and_then(|device| device.comment.as_deref())
                .and_then(friendly_device_name)
                .or_else(advertised)
                .unwrap_or_else(|| {
                    format!("Device {}", fingerprint.chars().take(8).collect::<String>())
                })
        } else {
            friendly_device_name(peer).unwrap_or_else(|| peer.to_owned())
        }
    }
    pub fn pause_caption(&self) -> &'static str {
        if self.connection == Connection::Paused {
            "Resume syncing"
        } else {
            "Pause syncing"
        }
    }
}

/// Fixed explanations for stable codes. Raw daemon text can carry paths or
/// clipboard fragments, so it is never shown once a code or a path marker exists.
fn user_facing_error(error: &ControlError) -> (Option<String>, String) {
    let code = error
        .code
        .as_deref()
        .filter(|code| error_code_token(code).is_some());
    let text = match code {
        Some(error_codes::HISTORY_UNAVAILABLE) => {
            "Clipboard history is unavailable. Restart the service or repair setup. History contents were not shown."
        }
        Some(error_codes::HISTORY_NOT_FOUND) => "That saved copy is no longer available.",
        Some(error_codes::HISTORY_QUERY_TOO_LONG) => {
            "Search is limited to 256 bytes. Shorten your search."
        }
        Some(error_codes::HISTORY_ENTRY_TOO_LARGE) | Some(error_codes::CLIPBOARD_TOO_LARGE) => {
            "That copy is larger than the configured limit."
        }
        Some(error_codes::CLIPBOARD_APPLIED_HISTORY_FAILED) => {
            "The clipboard changed, but this copy could not be saved to history."
        }
        Some(error_codes::CLIPBOARD_WRITE_UNKNOWN) => {
            "The clipboard may have changed. Check it before copying again."
        }
        Some(error_codes::IMAGE_PEER_UNSUPPORTED) => "Connected devices do not support image sync.",
        Some(error_codes::IMAGE_PEER_LIMIT) => "The image exceeds the connected devices’ limits.",
        Some(error_codes::IMAGE_QUEUE_FULL) => "The image queue is full. Try syncing again.",
        Some(error_codes::IMAGE_NOT_QUEUED) => "No connected device accepted the image. Check service logs for details.",
        Some(error_codes::SYNC_PAUSED) => "Sync is paused. Resume syncing to share the current copy.",
        Some(error_codes::RESUME_FAILED) => {
            "Sync could not resume, so it stays paused. Check the clipboard and try again."
        }
        Some(error_codes::UNSUPPORTED_PROTOCOL) | Some(error_codes::UNSUPPORTED_COMMAND) => {
            "This service needs an update before that action can run."
        }
        Some(error_codes::PEER_OTHER_USER) => {
            "The service belongs to another user. Choose the service for this account."
        }
        Some(error_codes::FRAME_TOO_LARGE) => "The service rejected an oversized request.",
        Some(error_codes::DAEMON_PAUSED) => "Sync is paused. Copies stay on this computer.",
        Some(error_codes::ENROLLMENT_CLOSED) => "This computer is not allowing a device right now.",
        Some(error_codes::ENROLLMENT_EXPIRED) => "Pairing took too long and stopped. Try again.",
        Some(error_codes::ENROLLMENT_CANCELLED) => "Pairing was cancelled.",
        Some(error_codes::ENROLLMENT_RATE_LIMITED) => {
            "Too many pairing attempts. Wait a minute and try again."
        }
        Some(error_codes::ENROLLMENT_MISMATCH) => {
            "These computers could not verify each other. Nothing was paired."
        }
        Some(error_codes::ENROLLMENT_REFLECTION) | Some(error_codes::ENROLLMENT_REPLAY) => {
            "That connection was refused. Nothing was paired."
        }
        Some(error_codes::ENROLLMENT_OVERSIZE) => {
            "The connection was refused because the message was too large."
        }
        Some(error_codes::ENROLLMENT_BUSY) => "Another pairing attempt is in progress. Try again in a moment.",
        Some(error_codes::ENROLLMENT_NOT_FOUND) => "That pairing attempt is no longer available.",
        Some(error_codes::ENROLLMENT_INCOMPLETE) => {
            "Pairing is not finished on both computers."
        }
        Some(error_codes::ENROLLMENT_UNAUTHORIZED) => "The service rejected that device action.",
        Some(error_codes::ENROLLMENT_ADDRESS_INVALID) => {
            "Type the address with its port, like 192.168.1.20:8485."
        }
        Some(_) => "The service could not complete that action. Try again.",
        None if error.message.to_ascii_lowercase().contains("timed out") => {
            "The service did not answer in time. Check that it is running, then try again."
        }
        None if error.message.to_ascii_lowercase().contains("another user") => {
            "The service belongs to another user. Choose the service for this account."
        }
        None => "The service could not complete that request. Check that it is running, then try again.",
    };
    (code.map(str::to_string), text.to_string())
}

fn error_code_token(text: &str) -> Option<&str> {
    if (1..=64).contains(&text.len())
        && text
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    {
        Some(text)
    } else {
        None
    }
}

/// Allowlisted diagnostics text. Returns None when the redaction marker is missing
/// or the rendered text still contains a path or format mark.
fn render_diagnostics(report: &DiagnosticsReport) -> Option<String> {
    if report.redaction != DIAGNOSTICS_REDACTION {
        return None;
    }
    let version = plain_token(&report.app_version, 32, true).unwrap_or_else(|| "unknown".into());
    let generated = plain_token(&report.generated_at, 40, true).unwrap_or_else(|| "unknown".into());
    let service = match report.service_mode_hint.as_str() {
        "user-launchd" | "user-systemd" | "foreground" | "unknown" => {
            report.service_mode_hint.as_str()
        }
        _ => "unknown",
    };
    let features: Vec<String> = report
        .features
        .iter()
        .filter_map(|feature| plain_token(feature, 32, false))
        .take(32)
        .collect();
    let labels: Vec<String> = report
        .config_labels
        .iter()
        .filter_map(|label| config_label(label))
        .take(32)
        .collect();
    let errors: Vec<String> = report
        .recent_errors
        .iter()
        .filter_map(diagnostic_error_line)
        .take(20)
        .collect();
    let mut lines = vec![
        format!("ClipSync diagnostics ({DIAGNOSTICS_REDACTION})"),
        format!("Generated: {generated}"),
        format!("Version: {version}"),
        format!("Control protocol: {}", report.control_version),
        format!("Paused: {}", if report.paused { "yes" } else { "no" }),
        format!("Connected devices: {}", report.connected_peer_count),
        format!(
            "History: {}",
            if report.history_available {
                "available"
            } else {
                "unavailable"
            }
        ),
        format!("History capacity: {}", report.history_capacity),
        format!("Clipboard limit: {} bytes", report.max_clipboard_bytes),
        format!("Service: {service}"),
        format!(
            "Features: {}",
            if features.is_empty() {
                "none".into()
            } else {
                features.join(", ")
            }
        ),
        format!(
            "Config: {}",
            if labels.is_empty() {
                "none".into()
            } else {
                labels.join(", ")
            }
        ),
        "Recent errors:".into(),
    ];
    if errors.is_empty() {
        lines.push("none".into());
    } else {
        lines.extend(errors);
    }
    let rendered = lines.join("\n");
    if rendered.chars().any(|ch| {
        ch == '/'
            || ch == '\\'
            || (ch.is_control() && ch != '\n')
            || super::model::is_unsafe_format(ch)
    }) {
        return None;
    }
    Some(rendered)
}

fn plain_token(text: &str, max: usize, dotted: bool) -> Option<String> {
    let cleaned = display_text(text, false);
    if cleaned.is_empty() || cleaned.chars().count() > max {
        return None;
    }
    let ok = cleaned.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || ch == '_'
            || (dotted && matches!(ch, '.' | '+' | '-' | ':' | 'T' | 'Z'))
    });
    ok.then_some(cleaned)
}

fn config_label(text: &str) -> Option<String> {
    let cleaned = display_text(text, false);
    if cleaned.len() > 80 {
        return None;
    }
    let (key, value) = cleaned.split_once('=')?;
    let ident = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    };
    if ident(key) && ident(value) {
        Some(cleaned)
    } else {
        None
    }
}

fn diagnostic_error_line(error: &DiagnosticError) -> Option<String> {
    let code = error_code_token(&display_text(&error.code, false))?.to_string();
    let at = plain_token(&error.at, 40, true);
    Some(match at {
        Some(at) => format!("- {at} {code}"),
        None => format!("- {code}"),
    })
}

fn show_label(text: &str) -> String {
    display_text(text, false)
}

const NEEDS_UPDATE: &str = "This service needs an update before that action can run.";

/// One line for the footer. Peers are counted as queued, not acknowledged.
fn removal_notice(action: Action, removal: &HistoryRemovalView) -> String {
    let devices = |count: u32| {
        format!(
            "{count} connected device{}",
            if count == 1 { "" } else { "s" }
        )
    };
    let mut parts = vec![match action {
        Action::Clear => format!(
            "Cleared {} saved cop{}",
            removal.removed,
            if removal.removed == 1 { "y" } else { "ies" }
        ),
        _ if removal.removed == 0 => "Already removed here".into(),
        _ => "Deleted".into(),
    }];
    if removal.clipboard_cleared {
        parts.push("clipboard cleared".into());
    } else if action == Action::Clear {
        parts.push("the clipboard could not be cleared".into());
    }
    if removal.peers_notified > 0 {
        parts.push(format!("sent to {}", devices(removal.peers_notified)));
    }
    if removal.peers_skipped > 0 {
        parts.push(format!(
            "not sent to {} (older ClipSync or busy)",
            devices(removal.peers_skipped)
        ));
    }
    parts.join(" · ")
}
#[derive(Debug)]
pub enum Intent {
    Navigate(Route),
    Refresh,
    Search(String),
    Select(Uuid),
    CopySelected,
    /// Delete one saved copy here and on connected devices. Renderers confirm first.
    DeleteEntry(Uuid),
    /// Clear all history and the clipboard here and on connected devices.
    ClearHistory,
    DetailPage(usize),
    SetPaused(bool),
    SyncNow,
    Diagnostics,
    ClearNotice,
    OpenEnrollment,
    DiscoverEnrollment,
    DeviceAddress(String),
    /// Digits typed so far; anything else is dropped and input stops at six.
    PairingCode(String),
    SelectSession(Uuid),
    /// Dial `address` with the typed code. The code is cleared once sent:
    /// each code is good for one attempt.
    Pair {
        address: String,
        expected_fingerprint: Option<String>,
        expected_node_id: Option<Uuid>,
    },
    CancelEnrollment(Option<Uuid>),
    RevokeDevice(String),
}
struct Job {
    ticket: Ticket,
    command: Command,
}
struct Reply {
    ticket: Ticket,
    result: Result<Response, control::ControlError>,
    action: Action,
}

/// The command an action reply answers, so its outcome can be applied and worded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Copy,
    Delete(Uuid),
    Clear,
    Other,
}
impl Action {
    fn of(command: &Command) -> Self {
        match command {
            Command::HistoryCopy { .. } | Command::Copy { .. } => Self::Copy,
            Command::HistoryDelete { id } => Self::Delete(*id),
            Command::HistoryClear => Self::Clear,
            _ => Self::Other,
        }
    }
}

pub struct Controller {
    pub view: View,
    jobs: Option<mpsc::SyncSender<Job>>,
    replies: mpsc::Receiver<Reply>,
    reconciliation: Reconciliation,
    last_poll: Instant,
    status_pending: bool,
    error_domain: Option<Domain>,
    enrollment_pending: bool,
    devices_pending: bool,
    history_generation: Option<(Uuid, u64)>,
    /// Connected peer names when trusted devices were last read for naming.
    trusted_checked: Option<Vec<String>>,
    /// Generation that produced the current success notice. A newer status clears it.
    notice_generation: Option<u64>,
    /// Read requests that could not be queued. At most one job per read domain,
    /// in the order that domain first waited. A later read replaces only its own
    /// domain, so a status poll cannot discard a parked detail or search.
    /// Mutations are not retried.
    retries: VecDeque<Job>,
}

fn is_read(domain: Domain) -> bool {
    matches!(
        domain,
        Domain::Status | Domain::History | Domain::Detail | Domain::Enrollment | Domain::Devices
    )
}

impl Controller {
    /// Starts a client only. This never starts a daemon or opens its history DB.
    pub fn connect(config: Config) -> anyhow::Result<Self> {
        let (jobs, incoming) = mpsc::sync_channel::<Job>(8);
        let (outgoing, replies) = mpsc::sync_channel::<Reply>(8);
        std::thread::Builder::new()
            .name("clipsync-companion-ipc".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => return,
                };
                let path = control::socket_path(&config);
                let max_frame = control::max_frame_bytes(&config);
                while let Ok(job) = incoming.recv() {
                    let action = Action::of(&job.command);
                    let result =
                        runtime.block_on(control::request_result(&path, job.command, max_frame));
                    if outgoing
                        .send(Reply {
                            ticket: job.ticket,
                            result,
                            action,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        let mut client = Self::with_channels(jobs, replies);
        client.refresh();
        Ok(client)
    }
    fn with_channels(jobs: mpsc::SyncSender<Job>, replies: mpsc::Receiver<Reply>) -> Self {
        Self {
            view: View::default(),
            jobs: Some(jobs),
            replies,
            reconciliation: Reconciliation::default(),
            last_poll: Instant::now(),
            status_pending: false,
            error_domain: None,
            enrollment_pending: false,
            devices_pending: false,
            history_generation: None,
            trusted_checked: None,
            notice_generation: None,
            retries: VecDeque::new(),
        }
    }
    pub fn dispatch(&mut self, intent: Intent) {
        match intent {
            Intent::Navigate(route) => {
                self.view.route = route;
                if route == Route::Devices {
                    self.refresh_devices();
                }
                if route == Route::History {
                    self.history();
                }
            }
            Intent::Refresh => {
                self.refresh();
                if self.view.route == Route::History {
                    self.history();
                }
            }
            Intent::Search(query) => {
                if query.len() > 256 {
                    self.view.error =
                        Some("Search is limited to 256 bytes. Shorten your search.".into());
                    self.view.error_code = Some(error_codes::HISTORY_QUERY_TOO_LONG.into());
                    return;
                }
                let unavailable = !matches!(
                    self.view.connection,
                    Connection::Ready | Connection::Connected(_) | Connection::Paused
                );
                self.view.query = query;
                self.reconciliation.begin(Domain::Detail);
                self.view.selected = None;
                self.view.detail = None;
                self.view.image_preview = None;
                self.view.detail_page = 0;
                self.view.detail_loading = false;
                self.view.history.clear();
                self.history_generation = None;
                if unavailable {
                    self.view.history_loading = false;
                    self.view.error = Some(
                        "The service cannot be reached, so search results were cleared. Try again when it is available."
                            .into(),
                    );
                    self.view.error_code = None;
                    return;
                }
                self.history();
            }
            Intent::DetailPage(page) => {
                self.view.detail_page = page.min(self.view.detail_pages() - 1);
            }
            Intent::Select(id) => {
                if self.view.history.iter().any(|row| row.id == id) {
                    self.view.selected = Some(id);
                    self.view.detail = None;
                    self.view.image_preview = None;
                    self.view.detail_page = 0;
                    let sent = self.send(Domain::Detail, Command::HistoryGet { id });
                    self.view.detail_loading = self.arm(Domain::Detail, sent);
                }
            }
            Intent::CopySelected
                if self.view.can_act()
                    && self.view.detail.is_some()
                    && !self.view.detail_loading =>
            {
                if let Some(id) = self.view.selected {
                    self.view.action_pending =
                        self.send(Domain::Action, Command::HistoryCopy { id });
                }
            }
            Intent::DeleteEntry(id) => {
                let supported = self.view.history_delete_supported;
                self.remove(supported, Command::HistoryDelete { id });
            }
            Intent::ClearHistory => {
                let supported = self.view.history_clear_supported;
                self.remove(supported, Command::HistoryClear);
            }
            Intent::SetPaused(paused) if self.view.can_act() => {
                self.view.action_pending = self.send(Domain::Action, Command::SetPause { paused });
            }
            Intent::SyncNow
                if self.view.can_act() && self.view.connection != Connection::Paused =>
            {
                self.view.action_pending = self.send(Domain::Action, Command::Sync);
            }
            Intent::Diagnostics if self.view.can_act() => {
                self.view.action_pending = self.send(Domain::Action, Command::DiagnosticsExport);
            }
            Intent::DeviceAddress(address) => {
                if address.len() <= 256 {
                    self.view.device_address = address;
                }
            }
            Intent::SelectSession(id) => {
                self.view.selected_session = Some(id);
            }
            Intent::OpenEnrollment if self.can_enroll() => {
                self.view.action_pending = self.send(Domain::Action, Command::EnrollmentOpen);
            }
            Intent::DiscoverEnrollment if self.can_enroll() => {
                self.view.action_pending = self.send(Domain::Action, Command::EnrollmentDiscover);
            }
            Intent::PairingCode(text) => {
                self.view.pairing_code = CodeEntry(
                    text.chars()
                        .filter(char::is_ascii_digit)
                        .take(PairingCode::DIGITS)
                        .collect(),
                );
            }
            Intent::Pair {
                address,
                expected_fingerprint,
                expected_node_id,
            } if self.can_enroll() => {
                let Some(code) = PairingCode::parse(self.view.pairing_code.as_str()) else {
                    self.view.error =
                        Some("Type the 6-digit code shown on the other computer.".into());
                    self.view.error_code = None;
                    return;
                };
                self.view.pairing_code = CodeEntry::default();
                self.view.action_pending = self.send(
                    Domain::Action,
                    Command::EnrollmentPair {
                        address,
                        code,
                        expected_fingerprint,
                        expected_node_id,
                    },
                );
            }
            Intent::CancelEnrollment(session_id) if self.can_enroll() => {
                self.view.action_pending =
                    self.send(Domain::Action, Command::EnrollmentCancel { session_id });
            }
            Intent::RevokeDevice(fingerprint)
                if self.view.can_act() && self.view.enrollment_supported =>
            {
                self.view.action_pending =
                    self.send(Domain::Action, Command::EnrollmentRevoke { fingerprint });
            }
            Intent::ClearNotice => {
                self.view.notice = None;
                self.view.error = None;
            }
            _ => {}
        }
    }
    /// Older services cannot parse removal commands and drop the connection
    /// unanswered, so they are never sent one.
    fn remove(&mut self, supported: bool, command: Command) {
        // Support is unknown until a status arrives, so readiness is checked first.
        let refusal = if !self.view.can_act() {
            "The service is busy or unavailable, so nothing was removed. Try again."
        } else if !supported {
            NEEDS_UPDATE
        } else {
            self.view.action_pending = self.send(Domain::Action, command);
            return;
        };
        self.view.error = Some(refusal.into());
        self.view.error_code = None;
    }
    fn deselect(&mut self) {
        self.reconciliation.begin(Domain::Detail);
        self.view.selected = None;
        self.view.detail = None;
        self.view.image_preview = None;
        self.view.detail_page = 0;
        self.view.detail_loading = false;
    }
    /// Show the result before the reloaded page arrives, keeping a neighbor
    /// selected: the row that moved into the deleted one's place, else the last.
    fn apply_removal(&mut self, action: Action, removal: &HistoryRemovalView) {
        match action {
            Action::Delete(id) => {
                if let Some(index) = self.view.history.iter().position(|row| row.id == id) {
                    self.view.history.remove(index);
                    if self.view.selected == Some(id) {
                        let next = self
                            .view
                            .history
                            .get(index)
                            .or(self.view.history.last())
                            .map(|row| row.id);
                        match next {
                            Some(next) => self.dispatch(Intent::Select(next)),
                            None => self.deselect(),
                        }
                    }
                }
            }
            Action::Clear => {
                self.view.history.clear();
                self.deselect();
            }
            Action::Copy | Action::Other => {}
        }
        self.view.notice = Some(removal_notice(action, removal));
        self.history();
    }
    fn send(&mut self, domain: Domain, command: Command) -> bool {
        let ticket = self.reconciliation.begin(domain);
        self.enqueue(Job { ticket, command })
    }
    fn enqueue(&mut self, job: Job) -> bool {
        let read = is_read(job.ticket.domain);
        match self.jobs.as_ref().map(|jobs| jobs.try_send(job)) {
            Some(Ok(())) => true,
            Some(Err(TrySendError::Full(job))) if read => {
                self.park_read(job);
                false
            }
            Some(Err(TrySendError::Full(_))) => {
                self.view.error =
                    Some("The service is busy. Try again after the current request.".into());
                false
            }
            _ => {
                self.view.error = Some(
                    "The service could not be reached. Start it from Settings, then try again."
                        .into(),
                );
                false
            }
        }
    }
    /// Keep the newest read for this domain. Other parked domains stay in place,
    /// so a later status or search cannot evict the latest detail.
    fn park_read(&mut self, job: Job) {
        if let Some(queued) = self
            .retries
            .iter_mut()
            .find(|queued| queued.ticket.domain == job.ticket.domain)
        {
            *queued = job;
            return;
        }
        self.retries.push_back(job);
    }
    fn arm(&self, domain: Domain, sent: bool) -> bool {
        sent || self
            .retries
            .iter()
            .any(|job| job.ticket.domain == domain && self.reconciliation.current(job.ticket))
    }
    fn note_read_pending(&mut self, domain: Domain) {
        match domain {
            Domain::Status => self.status_pending = true,
            Domain::History => self.view.history_loading = true,
            Domain::Detail => self.view.detail_loading = true,
            Domain::Enrollment => self.enrollment_pending = true,
            Domain::Devices => self.devices_pending = true,
            _ => {}
        }
    }
    fn flush_retry(&mut self) {
        let waiting = std::mem::take(&mut self.retries);
        let mut blocked = false;
        for job in waiting {
            if blocked {
                self.retries.push_back(job);
                continue;
            }
            // A newer request for this domain already owns the pending flag.
            if !self.reconciliation.current(job.ticket) {
                continue;
            }
            let domain = job.ticket.domain;
            if self.enqueue(job) {
                self.note_read_pending(domain);
            } else {
                // Full re-parks this read at the front. A dead worker does not.
                blocked = true;
            }
        }
    }
    /// Pairing actions need a service with code pairing; an older service is
    /// never sent a command it would parse as the removed fingerprint flow.
    pub fn can_enroll(&self) -> bool {
        self.view.can_act() && self.view.pairing_supported
    }
    fn refresh_devices(&mut self) {
        if !self.view.enrollment_supported {
            return;
        }
        if self.view.pairing_supported && !self.enrollment_pending {
            let sent = self.send(
                Domain::Enrollment,
                Command::EnrollmentInspect { session_id: None },
            );
            self.enrollment_pending = self.arm(Domain::Enrollment, sent);
        }
        if !self.devices_pending {
            let sent = self.send(Domain::Devices, Command::TrustedDevices);
            self.devices_pending = self.arm(Domain::Devices, sent);
        }
    }
    /// Authorization comments name connected peers on every screen. Read them
    /// again only when the set of connected peers changes.
    fn refresh_trusted(&mut self) {
        if !self.view.enrollment_supported || self.devices_pending {
            return;
        }
        let mut peers = self
            .view
            .devices
            .iter()
            .map(|device| device.name.clone())
            .collect::<Vec<_>>();
        peers.sort();
        if self.trusted_checked.as_ref() == Some(&peers) {
            return;
        }
        let sent = self.send(Domain::Devices, Command::TrustedDevices);
        self.devices_pending = self.arm(Domain::Devices, sent);
        if sent {
            self.trusted_checked = Some(peers);
        }
    }
    fn refresh(&mut self) {
        if !self.status_pending {
            let sent = self.send(Domain::Status, Command::Status);
            self.status_pending = self.arm(Domain::Status, sent);
            self.last_poll = Instant::now();
        }
    }
    fn history(&mut self) {
        if !matches!(
            self.view.connection,
            Connection::Ready | Connection::Connected(_) | Connection::Paused
        ) {
            return;
        }
        let command = if self.view.query.is_empty() {
            Command::HistoryList { limit: 20 }
        } else {
            Command::HistorySearch {
                query: self.view.query.clone(),
                limit: 20,
            }
        };
        let sent = self.send(Domain::History, command);
        self.view.history_loading = self.arm(Domain::History, sent);
    }
    pub fn tick(&mut self) {
        while let Ok(reply) = self.replies.try_recv() {
            self.apply(reply);
        }
        self.flush_retry();
        let now = Instant::now();
        self.view.stale = self.reconciliation.stale(now)
            || matches!(
                self.view.connection,
                Connection::Unavailable | Connection::UpgradeRequired
            );
        if now.duration_since(self.last_poll) >= Duration::from_secs(1) {
            self.refresh();
        }
    }
    fn apply(&mut self, reply: Reply) {
        if reply.ticket.domain == Domain::Status {
            self.status_pending = false;
        }
        if reply.ticket.domain == Domain::Enrollment {
            self.enrollment_pending = false;
        }
        if reply.ticket.domain == Domain::Devices {
            self.devices_pending = false;
        }
        if !self.reconciliation.current(reply.ticket) {
            return;
        }
        match reply.ticket.domain {
            Domain::Action => self.view.action_pending = false,
            Domain::History => self.view.history_loading = false,
            Domain::Detail => self.view.detail_loading = false,
            _ => {}
        }
        let response = match reply.result {
            Ok(response) => response,
            Err(error) => {
                self.reconciliation.failed(reply.ticket);
                if reply.ticket.domain == Domain::Action {
                    self.view.notice = None;
                    self.notice_generation = None;
                }
                if reply.ticket.domain == Domain::Status {
                    self.view.connection =
                        if error.code.as_deref() == Some(error_codes::UNSUPPORTED_PROTOCOL) {
                            Connection::UpgradeRequired
                        } else {
                            Connection::Unavailable
                        };
                    self.view.stale = true;
                }
                if reply.ticket.domain == Domain::History {
                    self.view.history.clear();
                    self.view.selected = None;
                    self.view.detail = None;
                    self.view.image_preview = None;
                    self.view.detail_page = 0;
                    self.view.detail_loading = false;
                    self.history_generation = None;
                    self.reconciliation.begin(Domain::Detail);
                }
                if reply.ticket.domain == Domain::Detail {
                    self.view.detail = None;
                    self.view.image_preview = None;
                    self.view.detail_page = 0;
                }
                let (code, text) = user_facing_error(&error);
                self.view.error_code = code;
                self.view.error = Some(text);
                self.error_domain = Some(reply.ticket.domain);
                return;
            }
        };
        if reply.ticket.domain != Domain::Status {
            if let (Some(instance), Some(generation)) =
                (response.instance_id, response.state_generation)
            {
                match self.reconciliation.note_generation(instance, generation) {
                    SnapshotOrder::Fresh => {}
                    SnapshotOrder::Stale | SnapshotOrder::OtherInstance => {
                        self.refresh();
                        return;
                    }
                }
            } else if response.instance_id.is_some()
                && self.reconciliation.instance_id().is_some()
                && response.instance_id != self.reconciliation.instance_id()
            {
                self.refresh();
                return;
            }
        }
        match reply.ticket.domain {
            Domain::Status => {
                let supports = |name: &str| {
                    response
                        .features
                        .as_ref()
                        .is_some_and(|features| features.iter().any(|feature| feature == name))
                };
                self.view.pairing_supported = supports(control::FEATURE_ENROLLMENT_CODE);
                self.view.enrollment_supported =
                    self.view.pairing_supported || supports(control::FEATURE_ENROLLMENT);
                self.view.history_delete_supported = supports(control::FEATURE_HISTORY_DELETE);
                self.view.history_clear_supported = supports(control::FEATURE_HISTORY_CLEAR);
                let compatible = response.features.as_ref().is_some_and(|features| {
                    ["pause", "history_read", "history_copy", "diagnostics"]
                        .iter()
                        .all(|needed| features.iter().any(|feature| feature == needed))
                });
                if !compatible {
                    self.view.connection = Connection::UpgradeRequired;
                    self.view.stale = true;
                    return;
                }
                let (Some(instance), Some(generation), Some(status)) = (
                    response.instance_id,
                    response.state_generation,
                    response.status,
                ) else {
                    self.view.connection = Connection::UpgradeRequired;
                    return;
                };
                let restarted = self
                    .reconciliation
                    .instance_id()
                    .is_some_and(|old| old != instance);
                if !self
                    .reconciliation
                    .status(reply.ticket, instance, generation, Instant::now())
                {
                    return;
                }
                if restarted {
                    self.view.action_pending = false;
                    self.view.history_loading = false;
                    self.view.detail_loading = false;
                    self.enrollment_pending = false;
                    self.devices_pending = false;
                    self.view.enrollment = None;
                    self.view.trusted_devices.clear();
                    self.trusted_checked = None;
                    self.view.selected_session = None;
                    self.view.history.clear();
                    self.view.selected = None;
                    self.view.detail = None;
                    self.view.image_preview = None;
                    self.view.detail_page = 0;
                    self.view.notice =
                        Some("The service restarted. State has been refreshed.".into());
                    self.notice_generation = Some(generation);
                }
                if self.error_domain == Some(Domain::Status) {
                    self.view.error = None;
                    self.view.error_code = None;
                    self.error_domain = None;
                }
                if self
                    .notice_generation
                    .is_some_and(|notice| generation > notice)
                    && !restarted
                {
                    self.view.notice = None;
                    self.notice_generation = None;
                }
                self.apply_live_status(status, response.peers);
                if self.view.route == Route::Devices {
                    self.refresh_devices();
                } else {
                    self.refresh_trusted();
                }
                // Refresh history on status polls while keeping typed search and selection.
                if self.view.route == Route::History
                    && !self.view.history_loading
                    && self.history_generation != Some((instance, generation))
                {
                    self.history();
                }
            }
            Domain::History => {
                self.history_generation = response.instance_id.zip(response.state_generation);
                if let Some(page) = response.history {
                    self.view.history = page
                        .entries
                        .into_iter()
                        .map(|entry| {
                            let copied_at = chrono::DateTime::parse_from_rfc3339(&entry.timestamp)
                                .ok()
                                .map(|at| at.with_timezone(&chrono::Local));
                            HistoryRow {
                                id: entry.id,
                                preview: display_text(&entry.preview, false),
                                timestamp: copied_at
                                    .map(|at| at.format("%b %-d · %-I:%M %p").to_string())
                                    .unwrap_or_else(|| show_label(&entry.timestamp)),
                                source: if entry.origin_node.to_string() == self.view.node_id {
                                    "This computer".into()
                                } else {
                                    self.view
                                        .devices
                                        .iter()
                                        .find(|peer| peer.id == entry.origin_node)
                                        .map(|peer| peer.name.clone())
                                        .unwrap_or_else(|| {
                                            format!(
                                                "Device {}",
                                                &entry.origin_node.to_string()[..8]
                                            )
                                        })
                                },
                                characters: entry.content_chars,
                                content_type: entry.content_type,
                                origin: entry.origin_node,
                                copied_at,
                            }
                        })
                        .collect();
                    if self
                        .view
                        .selected
                        .is_some_and(|id| !self.view.history.iter().any(|row| row.id == id))
                    {
                        self.reconciliation.begin(Domain::Detail);
                        self.view.selected = None;
                        self.view.detail = None;
                        self.view.image_preview = None;
                        self.view.detail_page = 0;
                        self.view.detail_loading = false;
                    }
                }
            }
            Domain::Detail => {
                if let Some(entry) = response.history_entry {
                    if self.view.selected == Some(entry.id) {
                        self.view.detail = Some(display_text(&entry.text, true));
                        self.view.image_preview =
                            entry.image_preview.filter(ImagePreview::is_valid);
                    }
                }
            }
            Domain::Enrollment => {
                if let Some(snapshot) = response.enrollment {
                    self.apply_enrollment(snapshot);
                }
            }
            Domain::Devices => {
                if let Some(devices) = response.trusted_devices {
                    self.apply_trusted(devices);
                }
            }
            Domain::Action => {
                if reply.action == Action::Copy && response.pause_applied.is_none() {
                    self.view.notice = None;
                    self.notice_generation = None;
                    self.view.error = Some("The service did not confirm the local copy. Update the service and try again.".into());
                    self.view.error_code = None;
                    return;
                }
                let removing = matches!(reply.action, Action::Delete(_) | Action::Clear);
                if removing && response.history_removal.is_none() {
                    self.view.notice = None;
                    self.notice_generation = None;
                    self.view.error = Some(
                        "The service did not confirm the removal. Check History, then try again."
                            .into(),
                    );
                    self.view.error_code = None;
                    self.history();
                    self.refresh();
                    return;
                }
                if let Some(status) = response.status.clone() {
                    self.apply_live_status(status, response.peers.clone());
                }
                self.view.error = None;
                self.view.error_code = None;
                self.error_domain = None;
                if let Some(snapshot) = response.enrollment {
                    self.apply_enrollment(snapshot);
                }
                if let Some(devices) = response.trusted_devices {
                    self.apply_trusted(devices);
                }
                let reported_diagnostics = response.diagnostics.is_some();
                if let Some(removal) = response.history_removal.filter(|_| removing) {
                    self.apply_removal(reply.action, &removal);
                    self.notice_generation = response.state_generation;
                } else if let Some(report) = response.diagnostics {
                    match render_diagnostics(&report) {
                        Some(text) => {
                            self.view.diagnostics = Some(text);
                            self.view.notice = Some(
                                "Diagnostics are ready. Clipboard contents and keys are excluded."
                                    .into(),
                            );
                            self.notice_generation = response.state_generation;
                        }
                        None => {
                            self.view.diagnostics = None;
                            self.view.error = Some(
                                "Diagnostics could not be verified and were not shown.".into(),
                            );
                            self.view.error_code = None;
                        }
                    }
                } else if let Some(paused) = response.pause_applied {
                    self.view.notice = Some(if response.queued.is_some() {
                        if paused {
                            "Copied locally. Sync is paused.".into()
                        } else {
                            format!(
                                "Copied locally; queued for {} connected device(s).",
                                response.queued.unwrap_or(0)
                            )
                        }
                    } else if paused {
                        "Paused until the service restarts.".into()
                    } else {
                        "Syncing resumed.".into()
                    });
                } else if let Some(queued) = response.queued {
                    self.view.notice = Some(format!("Queued for {queued} connected device(s)."));
                    self.notice_generation = response.state_generation;
                }
                if !reported_diagnostics && response.pause_applied.is_some() {
                    self.notice_generation = response.state_generation;
                }
                self.refresh();
            }
            _ => {}
        }
    }
    fn apply_live_status(&mut self, status: DaemonStatus, peers: Vec<crate::adapters::Peer>) {
        self.view.node_id = show_label(&status.node_id);
        self.view.connection = if status.paused {
            Connection::Paused
        } else if peers.is_empty() {
            Connection::Ready
        } else {
            Connection::Connected(peers.len())
        };
        self.view.devices = peers
            .into_iter()
            .map(|peer| DeviceRow {
                id: peer.id,
                name: show_label(&peer.hostname),
                address: show_label(&peer.address),
            })
            .collect();
        for device in &self.view.devices {
            self.view.seen_peers.insert(device.id, device.name.clone());
        }
        self.reconciliation.mark_fresh(Instant::now());
    }
    fn apply_trusted(&mut self, mut devices: Vec<TrustedDeviceView>) {
        for device in &mut devices {
            device.fingerprint = show_label(&device.fingerprint);
            device.comment = device.comment.take().map(|text| show_label(&text));
        }
        self.view.trusted_devices = devices;
    }
    fn apply_enrollment(&mut self, mut snapshot: EnrollmentSnapshot) {
        snapshot.listen_addr = snapshot.listen_addr.take().map(|addr| show_label(&addr));
        for hint in &mut snapshot.listen_hints {
            *hint = show_label(hint);
        }
        if let Some(outcome) = snapshot.outcome.as_mut() {
            outcome.status = show_label(&outcome.status);
            outcome.detail = outcome.detail.take().map(|detail| show_label(&detail));
            outcome.peer_fingerprint = outcome
                .peer_fingerprint
                .take()
                .map(|fingerprint| show_label(&fingerprint));
        }
        for session in &mut snapshot.sessions {
            session.role = show_label(&session.role);
            session.state = show_label(&session.state);
            session.local_fingerprint = show_label(&session.local_fingerprint);
            session.peer_fingerprint = session
                .peer_fingerprint
                .take()
                .map(|fingerprint| show_label(&fingerprint));
            session.dialed_addr = session.dialed_addr.take().map(|addr| show_label(&addr));
            session.peer_display_name = session
                .peer_display_name
                .take()
                .map(|name| show_label(&name));
            // Fixed text for known reasons; raw daemon text is never shown.
            session.error = session
                .failure
                .as_deref()
                .map(|failure| match control::PairingFailure::parse(failure) {
                    Some(known) => known.message().to_string(),
                    None => "Pairing could not be completed.".to_string(),
                })
                .or_else(|| {
                    session
                        .error
                        .take()
                        .map(|_| "Pairing could not be completed.".into())
                });
        }
        for candidate in &mut snapshot.candidates {
            candidate.display_name = show_label(&candidate.display_name);
            candidate.address = show_label(&candidate.address);
            candidate.source = show_label(&candidate.source);
            candidate.advertised_fingerprint = candidate
                .advertised_fingerprint
                .take()
                .map(|fingerprint| show_label(&fingerprint));
        }
        // A dial response identifies its own session, independent of HashMap
        // order or older failed attempts. Polls carry no focus hint.
        if let Some(id) = snapshot.requested_session_id {
            if snapshot
                .sessions
                .iter()
                .any(|session| session.session_id == id)
            {
                self.view.selected_session = Some(id);
            }
        }
        if self.view.selected_session.is_none_or(|id| {
            !snapshot
                .sessions
                .iter()
                .any(|session| session.session_id == id)
        }) {
            self.view.selected_session =
                snapshot.sessions.first().map(|session| session.session_id);
        }
        self.view.enrollment = Some(snapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE_FEATURES: [&str; 4] = ["pause", "history_read", "history_copy", "diagnostics"];

    /// A controller whose worker is the test: jobs are inspected, replies applied directly.
    fn ready(extra: &[&str]) -> (Controller, mpsc::Receiver<Job>) {
        let (jobs, incoming) = mpsc::sync_channel(8);
        let (_, replies) = mpsc::sync_channel(8);
        let mut client = Controller::with_channels(jobs, replies);
        client.refresh();
        let mut response = Response::base();
        response.features = Some(
            BASE_FEATURES
                .iter()
                .chain(extra)
                .map(|feature| feature.to_string())
                .collect(),
        );
        response.status = Some(DaemonStatus {
            paused: false,
            observed_at: String::new(),
            connected_peer_count: 0,
            history_available: true,
            node_id: Uuid::nil().to_string(),
            max_clipboard_bytes: 1024,
            history_capacity: 20,
            outbound_pending: false,
        });
        answer(&mut client, &incoming, 1, response);
        client.view.stale = false;
        (client, incoming)
    }

    /// Answers the oldest queued request and returns what it asked for.
    fn answer(
        client: &mut Controller,
        jobs: &mpsc::Receiver<Job>,
        generation: u64,
        mut response: Response,
    ) -> Command {
        let job = jobs.try_recv().expect("a queued request");
        response.instance_id = Some(Uuid::from_u128(99));
        response.state_generation = Some(generation);
        client.apply(Reply {
            ticket: job.ticket,
            action: Action::of(&job.command),
            result: Ok(response),
        });
        job.command
    }

    fn removed(
        removed: u32,
        clipboard_cleared: bool,
        peers_notified: u32,
        peers_skipped: u32,
    ) -> Response {
        let mut response = Response::base();
        response.history_removal = Some(HistoryRemovalView {
            removed,
            clipboard_cleared,
            peers_notified,
            peers_skipped,
        });
        response
    }

    fn with_rows(client: &mut Controller, count: u128) -> Vec<Uuid> {
        let ids = (1..=count).map(Uuid::from_u128).collect::<Vec<_>>();
        client.view.history = ids
            .iter()
            .map(|id| HistoryRow {
                id: *id,
                preview: format!("copy {id}"),
                timestamp: String::new(),
                source: String::new(),
                characters: 6,
                content_type: "text/plain".into(),
                origin: Uuid::nil(),
                copied_at: None,
            })
            .collect();
        ids
    }

    fn shown(client: &Controller) -> Vec<Uuid> {
        client.view.history.iter().map(|row| row.id).collect()
    }

    fn sent(jobs: &mpsc::Receiver<Job>) -> Vec<Command> {
        jobs.try_iter().map(|job| job.command).collect()
    }

    fn is_pairing(command: &Command) -> bool {
        matches!(
            command,
            Command::EnrollmentOpen
                | Command::EnrollmentDiscover
                | Command::EnrollmentPair { .. }
                | Command::EnrollmentInspect { .. }
                | Command::EnrollmentCancel { .. }
        )
    }

    #[test]
    fn a_service_without_code_pairing_is_never_sent_a_pairing_command() {
        // A fingerprint-compare service would parse an extra field and run the
        // old flow, so nothing about pairing reaches it.
        let (mut client, jobs) = ready(&["enrollment"]);
        assert!(client.view.enrollment_supported);
        assert!(!client.view.pairing_supported);
        client.dispatch(Intent::Navigate(Route::Devices));
        client.dispatch(Intent::OpenEnrollment);
        client.dispatch(Intent::DiscoverEnrollment);
        client.dispatch(Intent::PairingCode("123456".into()));
        client.dispatch(Intent::Pair {
            address: "127.0.0.1:8485".into(),
            expected_fingerprint: None,
            expected_node_id: None,
        });
        client.dispatch(Intent::CancelEnrollment(None));
        let sent = sent(&jobs);
        assert!(!sent.iter().any(is_pairing), "{sent:?}");
        assert!(sent.iter().any(|c| matches!(c, Command::TrustedDevices)));
    }

    #[test]
    fn a_typed_code_is_sent_once_and_never_shown_in_debug() {
        let (mut client, jobs) = ready(&["enrollment_code"]);
        assert!(client.view.pairing_supported && client.view.enrollment_supported);
        client.dispatch(Intent::PairingCode("48a2 9-13 77".into()));
        assert_eq!(client.view.pairing_code.as_str(), "482913");
        assert!(!format!("{:?}", client.view).contains("482913"));
        let pair = Intent::Pair {
            address: "127.0.0.1:8485".into(),
            expected_fingerprint: None,
            expected_node_id: None,
        };
        client.dispatch(pair);
        assert!(client.view.pairing_code.is_empty());
        let pairs: Vec<_> = sent(&jobs).into_iter().filter(is_pairing).collect();
        match pairs.as_slice() {
            [Command::EnrollmentPair { code, address, .. }] => {
                assert_eq!(code.digits(), "482913");
                assert_eq!(address, "127.0.0.1:8485");
            }
            other => panic!("{other:?}"),
        }
        client.view.action_pending = false;
        client.dispatch(Intent::Pair {
            address: "127.0.0.1:8485".into(),
            expected_fingerprint: None,
            expected_node_id: None,
        });
        assert!(
            !sent(&jobs).iter().any(is_pairing),
            "an empty code is not sent"
        );
        assert!(client
            .view
            .error
            .as_deref()
            .unwrap()
            .contains("6-digit code"));
    }

    #[test]
    fn session_errors_are_fixed_text_for_their_reason() {
        let (mut client, _jobs) = ready(&["enrollment_code"]);
        let session = |failure: Option<&str>, error: Option<&str>| control::EnrollmentSessionView {
            session_id: Uuid::new_v4(),
            role: "initiator".into(),
            state: "Failed".into(),
            peer_display_name: None,
            local_fingerprint: "SHA256:local".into(),
            peer_fingerprint: None,
            peer_node_id: None,
            dialed_addr: None,
            remaining_secs: 0,
            error: error.map(str::to_owned),
            failure: failure.map(str::to_owned),
        };
        client.apply_enrollment(EnrollmentSnapshot {
            requested_session_id: None,
            listening: false,
            listen_addr: None,
            listen_hints: vec![],
            expires_at: None,
            sessions: vec![
                session(Some("wrong_code"), Some("raw /Users/someone/path")),
                session(Some("not_a_reason"), None),
                session(None, Some("raw /Users/someone/path")),
            ],
            candidates: vec![],
            outcome: None,
            code: None,
        });
        let errors: Vec<_> = client
            .view
            .enrollment
            .as_ref()
            .unwrap()
            .sessions
            .iter()
            .map(|s| s.error.clone().unwrap())
            .collect();
        assert_eq!(errors[0], control::PairingFailure::WrongCode.message());
        assert_eq!(errors[1], "Pairing could not be completed.");
        assert_eq!(errors[2], "Pairing could not be completed.");
    }

    #[test]
    fn an_older_service_is_never_sent_a_removal() {
        let (mut client, jobs) = ready(&[]);
        assert!(!client.view.history_delete_supported);
        assert!(!client.view.history_clear_supported);
        assert_eq!(client.view.connection, Connection::Ready);
        client.dispatch(Intent::DeleteEntry(Uuid::from_u128(1)));
        client.dispatch(Intent::ClearHistory);
        assert!(sent(&jobs).is_empty());
        assert_eq!(client.view.error.as_deref(), Some(NEEDS_UPDATE));
        assert!(!client.view.action_pending);
    }

    #[test]
    fn a_removal_is_not_sent_while_the_service_is_stale() {
        let (mut client, jobs) = ready(&["history_delete", "history_clear"]);
        client.view.stale = true;
        client.dispatch(Intent::ClearHistory);
        assert!(sent(&jobs).is_empty());
        assert!(client
            .view
            .error
            .as_deref()
            .unwrap()
            .contains("nothing was removed"));

        // Before any status, support is unknown: say unavailable, not outdated.
        let (jobs_tx, jobs) = mpsc::sync_channel(8);
        let (_, replies) = mpsc::sync_channel(8);
        let mut unreached = Controller::with_channels(jobs_tx, replies);
        unreached.dispatch(Intent::DeleteEntry(Uuid::from_u128(1)));
        assert!(sent(&jobs).is_empty());
        assert!(unreached
            .view
            .error
            .as_deref()
            .unwrap()
            .contains("unavailable"));
    }

    #[test]
    fn deleting_keeps_a_neighbor_selected_and_reports_the_outcome() {
        let (mut client, jobs) = ready(&["history_delete", "history_clear"]);
        let [a, b, c] = with_rows(&mut client, 3)[..] else {
            unreachable!()
        };
        client.view.selected = Some(b);
        client.dispatch(Intent::DeleteEntry(b));
        assert!(client.view.action_pending);
        let command = answer(&mut client, &jobs, 2, removed(1, false, 1, 0));
        assert!(matches!(command, Command::HistoryDelete { id } if id == b));
        assert!(!client.view.action_pending);
        assert_eq!(shown(&client), [a, c]);
        assert_eq!(client.view.selected, Some(c));
        assert_eq!(
            client.view.notice.as_deref(),
            Some("Deleted · sent to 1 connected device")
        );
        let follow_up = sent(&jobs);
        assert!(follow_up
            .iter()
            .any(|command| matches!(command, Command::HistoryGet { id } if *id == c)));
        assert!(follow_up
            .iter()
            .any(|command| matches!(command, Command::HistoryList { .. })));

        // The last row falls back to the one above it.
        client.dispatch(Intent::DeleteEntry(c));
        answer(&mut client, &jobs, 3, removed(1, true, 0, 0));
        assert_eq!(shown(&client), [a]);
        assert_eq!(client.view.selected, Some(a));
        assert_eq!(
            client.view.notice.as_deref(),
            Some("Deleted · clipboard cleared")
        );
    }

    #[test]
    fn deleting_an_unselected_row_leaves_the_selection_alone() {
        let (mut client, jobs) = ready(&["history_delete"]);
        let [a, b] = with_rows(&mut client, 2)[..] else {
            unreachable!()
        };
        client.view.selected = Some(a);
        client.dispatch(Intent::DeleteEntry(b));
        answer(&mut client, &jobs, 2, removed(0, false, 0, 0));
        assert_eq!(shown(&client), [a]);
        assert_eq!(client.view.selected, Some(a));
        assert_eq!(client.view.notice.as_deref(), Some("Already removed here"));
    }

    #[test]
    fn clearing_empties_history_and_reports_partial_results() {
        let (mut client, jobs) = ready(&["history_clear"]);
        let ids = with_rows(&mut client, 3);
        client.view.selected = Some(ids[0]);
        client.view.detail = Some("copy".into());
        client.dispatch(Intent::ClearHistory);
        let command = answer(&mut client, &jobs, 2, removed(3, true, 1, 1));
        assert!(matches!(command, Command::HistoryClear));
        assert!(client.view.history.is_empty());
        assert_eq!(client.view.selected, None);
        assert_eq!(client.view.detail, None);
        assert_eq!(
            client.view.notice.as_deref(),
            Some("Cleared 3 saved copies · clipboard cleared · sent to 1 connected device · not sent to 1 connected device (older ClipSync or busy)")
        );
        assert!(sent(&jobs)
            .iter()
            .any(|command| matches!(command, Command::HistoryList { .. })));

        client.dispatch(Intent::ClearHistory);
        answer(&mut client, &jobs, 3, removed(1, false, 2, 0));
        assert_eq!(
            client.view.notice.as_deref(),
            Some("Cleared 1 saved copy · the clipboard could not be cleared · sent to 2 connected devices")
        );
    }

    #[test]
    fn a_removal_reply_without_a_result_is_an_error() {
        let (mut client, jobs) = ready(&["history_delete"]);
        let ids = with_rows(&mut client, 2);
        client.view.selected = Some(ids[0]);
        client.dispatch(Intent::DeleteEntry(ids[0]));
        answer(&mut client, &jobs, 2, Response::base());
        assert_eq!(client.view.notice, None);
        assert!(client
            .view
            .error
            .as_deref()
            .unwrap()
            .contains("did not confirm the removal"));
        assert_eq!(shown(&client), ids);
    }

    #[test]
    fn device_names_prefer_authorization_comments_and_survive_disconnects() {
        let fingerprint = "SHA256:Jhs4k2abcdefghijklmnopqrstuvwxyz0123456789AB";
        let local = Uuid::from_u128(1);
        let peer = Uuid::from_u128(0xabcdef01 << 96);
        let mut view = View {
            node_id: local.to_string(),
            devices: vec![DeviceRow {
                id: peer,
                name: fingerprint.into(),
                address: "10.0.0.2:8484".into(),
            }],
            ..View::default()
        };
        assert_eq!(view.device_name(local), "This computer");
        assert_eq!(view.device_name(peer), "Device Jhs4k2ab");
        view.trusted_devices = vec![TrustedDeviceView {
            fingerprint: fingerprint.into(),
            comment: Some(format!("workbench-clipsync {fingerprint}")),
        }];
        assert_eq!(view.device_name(peer), "workbench");
        view.seen_peers.insert(peer, fingerprint.into());
        view.devices.clear();
        assert_eq!(view.device_name(peer), "workbench");
        assert_eq!(view.device_name(Uuid::from_u128(7)), "Device 00000000");
    }
    #[test]
    fn large_unicode_detail_pages_preserve_every_character() {
        let original = "a🙂é漢\n".repeat(4_000);
        let mut view = View {
            detail: Some(original.clone()),
            ..View::default()
        };
        let mut reconstructed = String::new();
        for page in 0..view.detail_pages() {
            view.detail_page = page;
            let slice = view.visible_detail().unwrap();
            assert!(slice.len() <= 8195);
            reconstructed.push_str(slice);
        }
        assert_eq!(reconstructed, original);
        view.detail_page = usize::MAX;
        assert!(!view.visible_detail().unwrap().is_empty());
    }

    #[test]
    fn diagnostics_renderer_drops_payloads_paths_and_format_marks() {
        let report = DiagnosticsReport {
            generated_at: "2026-10-20T00:00:00Z".into(),
            app_version: "0.1.0\u{202e}".into(),
            control_version: 1,
            features: vec!["pause".into(), "not a feature".into()],
            paused: true,
            connected_peer_count: 0,
            history_available: false,
            history_capacity: 20,
            max_clipboard_bytes: 1024,
            service_mode_hint: "foreground".into(),
            config_labels: vec![
                "history_key=configured".into(),
                "authorized_keys=/Users/me/.ssh/authorized_keys".into(),
            ],
            recent_errors: vec![DiagnosticError {
                at: "2026-10-20T00:00:01Z".into(),
                code: "history_unavailable".into(),
                message: "History unavailable: secret-clipboard /tmp/history.key".into(),
            }],
            redaction: DIAGNOSTICS_REDACTION.into(),
        };
        let text = render_diagnostics(&report).unwrap();
        assert!(text.contains(DIAGNOSTICS_REDACTION));
        assert!(text.contains("history_key=configured"));
        assert!(text.contains("history_unavailable"));
        assert!(text.contains("Paused: yes"));
        assert!(!text.contains("secret-clipboard"), "{text}");
        assert!(!text.contains("/Users"), "{text}");
        assert!(!text.contains("history.key"), "{text}");
        assert!(!text.contains('\u{202e}'), "{text}");
        assert!(!text.contains("not a feature"), "{text}");

        let mut rejected = report.clone();
        rejected.redaction = "raw".into();
        assert!(render_diagnostics(&rejected).is_none());
    }

    #[test]
    fn image_partial_outcomes_are_distinct_and_redacted() {
        for (code, phrase) in [
            (
                error_codes::CLIPBOARD_APPLIED_HISTORY_FAILED,
                "could not be saved",
            ),
            (error_codes::CLIPBOARD_WRITE_UNKNOWN, "may have changed"),
        ] {
            let (_, text) = user_facing_error(&ControlError {
                code: Some(code.into()),
                message: "private-path".into(),
            });
            assert!(text.contains(phrase));
            assert!(!text.contains("private-path"));
        }
    }

    #[test]
    fn error_text_follows_the_code_and_hides_paths() {
        let error = ControlError {
            code: Some(error_codes::HISTORY_UNAVAILABLE.into()),
            message: "History unavailable: secret-clipboard /tmp/history.key".into(),
        };
        let (code, text) = user_facing_error(&error);
        assert_eq!(code.as_deref(), Some(error_codes::HISTORY_UNAVAILABLE));
        assert!(text.contains("unavailable"));
        assert!(!text.contains("secret-clipboard"), "{text}");
        assert!(!text.contains("history.key"), "{text}");

        let leaked = ControlError {
            code: None,
            message: "expected ident at line 1 column 2".into(),
        };
        let (_, text) = user_facing_error(&leaked);
        assert!(!text.contains("expected ident"), "{text}");
        assert!(!text.contains("column"), "{text}");
        assert!(text.contains("could not complete"));

        let plaintext = ControlError {
            code: None,
            message: "secret-clipboard".into(),
        };
        let (_, text) = user_facing_error(&plaintext);
        assert!(!text.contains("secret-clipboard"), "{text}");
        assert!(text.contains("could not complete"));
    }
}
