//! Keyboard interactions for the same enrollment intents as the desktop.
use super::{
    client::{Controller, Intent},
    terminal_theme::{Theme, Tone},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

#[derive(Default)]
pub struct DevicesUi {
    editing: bool,
    candidate: usize,
    trusted: usize,
    revoke: Option<String>,
}
impl DevicesUi {
    pub fn key(&mut self, key: KeyEvent, client: &mut Controller) -> bool {
        if self.editing {
            let mut address = client.view.device_address.clone();
            match key.code {
                KeyCode::Esc => self.editing = false,
                KeyCode::Enter => {
                    self.editing = false;
                    client.dispatch(Intent::DialEnrollment {
                        address,
                        expected_fingerprint: None,
                        expected_node_id: None,
                    });
                    return true;
                }
                KeyCode::Backspace => {
                    address.pop();
                }
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    address.push(ch)
                }
                _ => {}
            }
            client.dispatch(Intent::DeviceAddress(address));
            return true;
        }
        if let Some(fingerprint) = self.revoke.take() {
            if key.code == KeyCode::Char('y') {
                client.dispatch(Intent::RevokeDevice(fingerprint));
            }
            return true;
        }
        match key.code {
            KeyCode::Char('e') => client.dispatch(Intent::OpenEnrollment),
            KeyCode::Char('f') => client.dispatch(Intent::DiscoverEnrollment),
            KeyCode::Char('m') => self.editing = true,
            KeyCode::Char('y') => {
                if let Some(id) = client.view.selected_session {
                    client.dispatch(Intent::ConfirmEnrollment(id));
                }
            }
            KeyCode::Char('c') => {
                if let Some(id) = client.view.selected_session {
                    client.dispatch(Intent::CancelEnrollment(Some(id)));
                }
            }
            KeyCode::Char('x') => client.dispatch(Intent::CancelEnrollment(None)),
            KeyCode::Char('[') | KeyCode::Char(']') => {
                if let Some(snapshot) = &client.view.enrollment {
                    let index = snapshot
                        .sessions
                        .iter()
                        .position(|s| Some(s.session_id) == client.view.selected_session)
                        .unwrap_or(0);
                    let next = if key.code == KeyCode::Char('[') {
                        index.saturating_sub(1)
                    } else {
                        (index + 1).min(snapshot.sessions.len().saturating_sub(1))
                    };
                    if let Some(session) = snapshot.sessions.get(next) {
                        client.dispatch(Intent::SelectSession(session.session_id));
                    }
                }
            }
            KeyCode::Up => self.candidate = self.candidate.saturating_sub(1),
            KeyCode::Down => self.candidate = self.candidate.saturating_add(1),
            KeyCode::Enter => {
                if let Some(candidate) = client
                    .view
                    .enrollment
                    .as_ref()
                    .and_then(|s| s.candidates.get(self.candidate))
                    .cloned()
                {
                    client.dispatch(Intent::DialEnrollment {
                        address: candidate.address,
                        expected_fingerprint: candidate.advertised_fingerprint,
                        expected_node_id: candidate.advertised_node_id,
                    });
                }
            }
            KeyCode::Char('j') => self.trusted = self.trusted.saturating_add(1),
            KeyCode::Char('k') => self.trusted = self.trusted.saturating_sub(1),
            KeyCode::Char('D') => {
                self.revoke = client
                    .view
                    .trusted_devices
                    .get(self.trusted)
                    .map(|d| d.fingerprint.clone());
            }
            _ => return false,
        }
        self.candidate = self.candidate.min(
            client
                .view
                .enrollment
                .as_ref()
                .map_or(0, |s| s.candidates.len().saturating_sub(1)),
        );
        self.trusted = self
            .trusted
            .min(client.view.trusted_devices.len().saturating_sub(1));
        true
    }
    pub fn lines(&self, client: &Controller, theme: &Theme) -> Vec<Line<'static>> {
        let view = &client.view;
        let key = |text: &str| Span::styled(text.to_owned(), theme.key());
        let quiet = |text: &str| Span::styled(text.to_owned(), theme.muted());
        let heading = |text: &str| Span::styled(text.to_owned(), theme.heading());
        let strong =
            |text: String| Span::styled(text, Style::default().add_modifier(Modifier::BOLD));
        let marker = |selected: bool| {
            if selected {
                theme.selection_bar()
            } else {
                Span::raw(" ")
            }
        };
        let mut lines = vec![
            Line::from(vec![
                key("e"),
                Span::raw(" allow a device  "),
                key("f"),
                Span::raw(" find nearby  "),
                key("m"),
                Span::raw(" enter address  "),
                key("x"),
                Span::raw(" close enrollment"),
            ]),
            Line::from(vec![
                Span::styled("▲ ", theme.tone(Tone::Warn)),
                Span::raw("Compare BOTH full fingerprints on BOTH computers before confirming."),
            ]),
            Line::from(""),
        ];
        if !view.enrollment_supported {
            lines.push(Line::styled(
                "Update the service to add devices here.",
                theme.tone(Tone::Warn),
            ));
        }
        if self.editing {
            lines.push(Line::from(vec![
                quiet("Address: "),
                strong(view.device_address.clone()),
                Span::styled("▏", theme.accent()),
                quiet(" · "),
                key("Enter"),
                quiet(" connect / "),
                key("Esc"),
                quiet(" cancel"),
            ]));
        }
        if let Some(snapshot) = &view.enrollment {
            if snapshot.listening {
                lines.push(Line::from(vec![
                    Span::styled("● ", theme.tone(Tone::Good)),
                    Span::raw("Allowing connections: "),
                    strong(
                        snapshot
                            .listen_addr
                            .clone()
                            .unwrap_or_else(|| "opening…".into()),
                    ),
                ]));
            }
            if let Some(session) = snapshot
                .sessions
                .iter()
                .find(|s| Some(s.session_id) == view.selected_session)
            {
                lines.extend([
                    Line::from(vec![
                        Span::styled(
                            super::model::enrollment_state_label(&session.state).to_owned(),
                            theme.tone(Tone::Info),
                        ),
                        quiet(&format!(" · {}s left · ", session.remaining_secs)),
                        key("[ / ]"),
                        quiet(" switch session"),
                    ]),
                    Line::from(quiet("This computer:")),
                    Line::from(strong(session.local_fingerprint.clone())),
                    Line::from(quiet("Other computer:")),
                    Line::from(match &session.peer_fingerprint {
                        Some(fingerprint) => strong(fingerprint.clone()),
                        None => quiet("Waiting for secure connection…"),
                    }),
                    Line::from(if session.local_confirmed {
                        vec![
                            quiet("Confirmed here; awaiting both computers. · "),
                            key("c"),
                            quiet(" cancel"),
                        ]
                    } else {
                        vec![
                            key("y"),
                            Span::raw(" fingerprints match, confirm · "),
                            key("c"),
                            quiet(" cancel"),
                        ]
                    }),
                ]);
                if let Some(error) = &session.error {
                    lines.push(Line::styled(format!("✗ {error}"), theme.tone(Tone::Bad)));
                }
            }
            if let Some(outcome) = &snapshot.outcome {
                lines.push(Line::styled(
                    format!(
                        "{}: {}",
                        outcome.status,
                        outcome.detail.as_deref().unwrap_or_default()
                    ),
                    theme.tone(Tone::Info),
                ));
            }
            if !snapshot.candidates.is_empty() {
                lines.push(Line::default());
                lines.push(Line::from(vec![
                    heading("Nearby"),
                    quiet(" · "),
                    key("↑/↓"),
                    quiet(" select · "),
                    key("Enter"),
                    quiet(" connect"),
                ]));
            }
            for (index, candidate) in snapshot.candidates.iter().enumerate() {
                let hue = candidate
                    .advertised_node_id
                    .map_or_else(Style::default, |id| theme.device(id));
                lines.push(Line::from(vec![
                    marker(index == self.candidate),
                    Span::raw(" "),
                    Span::styled(candidate.display_name.clone(), hue),
                    quiet(&format!(" · {}", candidate.address)),
                ]));
            }
        }
        if lines.last().is_some_and(|line| line.width() > 0) {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(heading("Connected now")));
        if view.devices.is_empty() {
            lines.push(Line::styled(
                "No authenticated devices connected.",
                theme.muted(),
            ));
        }
        for device in &view.devices {
            lines.push(Line::from(vec![
                Span::styled("● ", theme.tone(Tone::Good)),
                Span::styled(view.device_name(device.id), theme.device(device.id)),
                quiet(&format!(" · {}", device.address)),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            heading("Authorized devices"),
            quiet(" · "),
            key("j/k"),
            quiet(" select · "),
            key("D"),
            quiet(" remove authorization"),
        ]));
        if view.trusted_devices.is_empty() {
            lines.push(Line::styled("No devices authorized yet.", theme.muted()));
        }
        for (index, device) in view.trusted_devices.iter().enumerate() {
            let connected = view
                .devices
                .iter()
                .find(|peer| peer.name == device.fingerprint);
            let name = device
                .comment
                .as_deref()
                .and_then(super::model::friendly_device_name)
                .unwrap_or_else(|| "Device".into());
            let selected = index == self.trusted;
            let mut row = vec![
                marker(selected),
                Span::raw(" "),
                Span::styled(
                    name,
                    connected.map_or_else(Style::default, |peer| theme.device(peer.id)),
                ),
                Span::raw("  "),
            ];
            row.extend(if connected.is_some() {
                [
                    Span::styled("● ", theme.tone(Tone::Good)),
                    Span::styled("connected", theme.tone(Tone::Good)),
                ]
            } else {
                [quiet("○ "), quiet("not connected")]
            });
            lines.push(Line::from(row));
            lines.push(Line::from(vec![
                marker(selected),
                Span::raw(" "),
                quiet(&device.fingerprint),
            ]));
        }
        if let Some(fingerprint) = &self.revoke {
            // Put destructive confirmation first so it remains visible in a small terminal.
            lines.splice(
                0..0,
                [
                    Line::from(vec![
                        Span::styled("REMOVE authorization?", theme.badge(Tone::Bad)),
                        Span::raw(" y confirms; any other key cancels."),
                    ]),
                    Line::from(strong(fingerprint.clone())),
                    Line::default(),
                ],
            );
        }
        lines
    }
}
