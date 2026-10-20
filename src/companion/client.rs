//! A bounded background IPC worker shared by the GUI and TUI.
use super::model::{display_text, Connection, Domain, Reconciliation, Route, Ticket};
use crate::{
    config::Config,
    control::{self, Command, EnrollmentSnapshot, Response, TrustedDeviceView},
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use uuid::Uuid;

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
                    return;
                }
                self.view.query = query;
                self.reconciliation.begin(Domain::Detail);
                self.view.selected = None;
                self.view.detail = None;
                self.view.detail_page = 0;
                self.view.detail_loading = false;
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
                    self.view.detail_loading =
                        self.send(Domain::Detail, Command::HistoryGet { id });
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
        match self
            .jobs
            .as_ref()
            .map(|jobs| jobs.try_send(Job { ticket, command }))
        {
            Some(Ok(())) => true,
            _ => {
                self.view.error =
                    Some("The service is busy. Try again after the current request.".into());
                false
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
            self.enrollment_pending = self.send(
                Domain::Enrollment,
                Command::EnrollmentInspect { session_id: None },
            );
        }
        if !self.devices_pending {
            self.devices_pending = self.send(Domain::Devices, Command::TrustedDevices);
        }
    }
    fn refresh(&mut self) {
        if !self.status_pending {
            self.status_pending = self.send(Domain::Status, Command::Status);
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
        self.view.history_loading = self.send(Domain::History, command);
    }
    pub fn tick(&mut self) {
        while let Ok(reply) = self.replies.try_recv() {
            self.apply(reply);
        }
        let now = Instant::now();
        self.view.stale = self.reconciliation.stale(now);
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
                    self.view.connection = Connection::Unavailable;
                    self.view.stale = true;
                }
                self.view.error_code = error.code;
                self.view.error = Some(display_text(&error.message, false));
                self.error_domain = Some(reply.ticket.domain);
                return;
            }
        };
        if reply.ticket.domain != Domain::Status
            && response.instance_id.is_some()
            && self.reconciliation.instance_id().is_some()
            && response.instance_id != self.reconciliation.instance_id()
        {
            self.refresh();
            return;
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
                }
                if self.error_domain == Some(Domain::Status) {
                    self.view.error = None;
                    self.error_domain = None;
                }
                self.view.node_id = status.node_id.to_string();
                self.view.connection = if status.paused {
                    Connection::Paused
                } else if response.peers.is_empty() {
                    Connection::Ready
                } else {
                    Connection::Connected(response.peers.len())
                };
                self.view.devices = response
                    .peers
                    .into_iter()
                    .map(|peer| DeviceRow {
                        id: peer.id,
                        name: display_text(&peer.hostname, false),
                        address: display_text(&peer.address, false),
                    })
                    .collect();
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
                                .unwrap_or(entry.timestamp),
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
                    return;
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
                if let Some(report) = response.diagnostics {
                    self.view.diagnostics = serde_json::to_string_pretty(&report).ok();
                    self.view.notice = Some(
                        "Diagnostics are ready. Clipboard contents and keys are excluded.".into(),
                    );
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
                }
                self.refresh();
            }
            _ => {}
        }
    }
    fn apply_trusted(&mut self, mut devices: Vec<TrustedDeviceView>) {
        for device in &mut devices {
            device.comment = device.comment.take().map(|text| display_text(&text, false));
        }
        self.view.trusted_devices = devices;
    }
    fn apply_enrollment(&mut self, mut snapshot: EnrollmentSnapshot) {
        for session in &mut snapshot.sessions {
            session.peer_display_name = session
                .peer_display_name
                .take()
                .map(|name| display_text(&name, false));
            session.error = session
                .error
                .take()
                .map(|error| display_text(&error, false));
        }
        for candidate in &mut snapshot.candidates {
            candidate.display_name = display_text(&candidate.display_name, false);
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
}
