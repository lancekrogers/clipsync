//! A bounded background IPC worker shared by the GUI and TUI.
use super::model::{
    display_text, Connection, Domain, Reconciliation, Route, SnapshotOrder, Ticket,
};
use crate::{
    config::Config,
    control::{
        self, error_codes, Command, ControlError, DaemonStatus, DiagnosticError, DiagnosticsReport,
        EnrollmentSnapshot, Response, TrustedDeviceView,
    },
};
use std::{
    collections::VecDeque,
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
}
#[derive(Clone, Debug)]
pub struct DeviceRow {
    pub id: Uuid,
    pub name: String,
    pub address: String,
}
#[derive(Clone, Debug)]
pub struct View {
    pub route: Route,
    pub connection: Connection,
    pub stale: bool,
    pub devices: Vec<DeviceRow>,
    pub node_id: String,
    pub enrollment_supported: bool,
    pub enrollment: Option<EnrollmentSnapshot>,
    pub trusted_devices: Vec<TrustedDeviceView>,
    pub device_address: String,
    pub selected_session: Option<Uuid>,
    pub history: Vec<HistoryRow>,
    pub query: String,
    pub selected: Option<Uuid>,
    pub detail: Option<String>,
    pub detail_page: usize,
    pub notice: Option<String>,
    pub error: Option<String>,
    pub error_code: Option<String>,
    pub diagnostics: Option<String>,
    pub action_pending: bool,
    pub history_loading: bool,
    pub detail_loading: bool,
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
            enrollment: None,
            trusted_devices: Vec::new(),
            device_address: String::new(),
            selected_session: None,
            history: Vec::new(),
            query: String::new(),
            selected: None,
            detail: None,
            detail_page: 0,
            notice: None,
            error: None,
            error_code: None,
            diagnostics: None,
            action_pending: false,
            history_loading: false,
            detail_loading: false,
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
        Some(error_codes::ENROLLMENT_CLOSED) => "Enrollment is not open. Allow a device, then try again.",
        Some(error_codes::ENROLLMENT_EXPIRED) => "Verification expired. Start the connection again.",
        Some(error_codes::ENROLLMENT_CANCELLED) => "The connection was cancelled.",
        Some(error_codes::ENROLLMENT_RATE_LIMITED) => {
            "Too many connection attempts. Wait and try again."
        }
        Some(error_codes::ENROLLMENT_MISMATCH) => {
            "These computers could not verify each other. No device was authorized."
        }
        Some(error_codes::ENROLLMENT_REFLECTION) | Some(error_codes::ENROLLMENT_REPLAY) => {
            "That connection was refused. No device was authorized."
        }
        Some(error_codes::ENROLLMENT_OVERSIZE) => {
            "The connection was refused because the message was too large."
        }
        Some(error_codes::ENROLLMENT_BUSY) => "A connection is already in progress.",
        Some(error_codes::ENROLLMENT_NOT_FOUND) => "That connection is no longer available.",
        Some(error_codes::ENROLLMENT_INCOMPLETE) => {
            "Authorization is not finished on both computers."
        }
        Some(error_codes::ENROLLMENT_UNAUTHORIZED) => "The service rejected that device action.",
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
#[derive(Debug)]
pub enum Intent {
    Navigate(Route),
    Refresh,
    Search(String),
    Select(Uuid),
    CopySelected,
    DetailPage(usize),
    SetPaused(bool),
    SyncNow,
    Diagnostics,
    ClearNotice,
    OpenEnrollment,
    DiscoverEnrollment,
    DeviceAddress(String),
    SelectSession(Uuid),
    DialEnrollment {
        address: String,
        expected_fingerprint: Option<String>,
        expected_node_id: Option<Uuid>,
    },
    ConfirmEnrollment(Uuid),
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
    copy_action: bool,
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
                    let copy_action = matches!(
                        &job.command,
                        Command::HistoryCopy { .. } | Command::Copy { .. }
                    );
                    let result =
                        runtime.block_on(control::request_result(&path, job.command, max_frame));
                    if outgoing
                        .send(Reply {
                            ticket: job.ticket,
                            result,
                            copy_action,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        let mut client = Self {
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
            notice_generation: None,
            retries: VecDeque::new(),
        };
        client.refresh();
        Ok(client)
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
            Intent::DialEnrollment {
                address,
                expected_fingerprint,
                expected_node_id,
            } if self.can_enroll() => {
                self.view.action_pending = self.send(
                    Domain::Action,
                    Command::EnrollmentDial {
                        address,
                        expected_fingerprint,
                        expected_node_id,
                    },
                );
            }
            Intent::ConfirmEnrollment(session_id) if self.can_enroll() => {
                let verified = self.view.enrollment.as_ref().is_some_and(|snapshot| {
                    snapshot.sessions.iter().any(|session| {
                        session.session_id == session_id
                            && session.peer_fingerprint.is_some()
                            && matches!(
                                session.state.as_str(),
                                "AwaitingLocalConfirm" | "AwaitingPeerConfirm"
                            )
                            && !session.local_confirmed
                            && session.remaining_secs > 0
                    })
                });
                if verified {
                    self.view.action_pending =
                        self.send(Domain::Action, Command::EnrollmentConfirm { session_id });
                }
            }
            Intent::CancelEnrollment(session_id) if self.can_enroll() => {
                self.view.action_pending =
                    self.send(Domain::Action, Command::EnrollmentCancel { session_id });
            }
            Intent::RevokeDevice(fingerprint) if self.can_enroll() => {
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
    pub fn can_enroll(&self) -> bool {
        self.view.can_act() && self.view.enrollment_supported
    }
    fn refresh_devices(&mut self) {
        if !self.view.enrollment_supported {
            return;
        }
        if !self.enrollment_pending {
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
                    self.view.detail_page = 0;
                    self.view.detail_loading = false;
                    self.history_generation = None;
                    self.reconciliation.begin(Domain::Detail);
                }
                if reply.ticket.domain == Domain::Detail {
                    self.view.detail = None;
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
                self.view.enrollment_supported = response
                    .features
                    .as_ref()
                    .is_some_and(|features| features.iter().any(|feature| feature == "enrollment"));
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
                    self.view.selected_session = None;
                    self.view.history.clear();
                    self.view.selected = None;
                    self.view.detail = None;
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
                        .map(|entry| HistoryRow {
                            id: entry.id,
                            preview: display_text(&entry.preview, false),
                            timestamp: chrono::DateTime::parse_from_rfc3339(&entry.timestamp)
                                .map(|at| {
                                    at.with_timezone(&chrono::Local)
                                        .format("%b %-d · %-I:%M %p")
                                        .to_string()
                                })
                                .unwrap_or_else(|_| show_label(&entry.timestamp)),
                            source: if entry.origin_node.to_string() == self.view.node_id {
                                "This computer".into()
                            } else {
                                self.view
                                    .devices
                                    .iter()
                                    .find(|peer| peer.id == entry.origin_node)
                                    .map(|peer| peer.name.clone())
                                    .unwrap_or_else(|| {
                                        format!("Device {}", &entry.origin_node.to_string()[..8])
                                    })
                            },
                            characters: entry.content_chars,
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
                        self.view.detail_page = 0;
                        self.view.detail_loading = false;
                    }
                }
            }
            Domain::Detail => {
                if let Some(entry) = response.history_entry {
                    if self.view.selected == Some(entry.id) {
                        self.view.detail = Some(display_text(&entry.text, true));
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
                if reply.copy_action && response.pause_applied.is_none() {
                    self.view.error = Some("The service did not confirm the local copy. Update the service and try again.".into());
                    self.view.error_code = None;
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
                if let Some(report) = response.diagnostics {
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
            session.error = session.error.take().map(|error| show_label(&error));
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
