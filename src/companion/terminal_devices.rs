//! Keyboard interactions for the same pairing intents as the desktop.
use super::{
    client::{Controller, Intent},
    model::{big_digits, countdown, enrollment_state_label, friendly_device_name},
    terminal_theme::{Theme, Tone},
};
use crate::control::{EnrollmentCandidateView, PairingCode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use uuid::Uuid;

/// Where the typed code goes.
#[derive(Clone)]
struct Target {
    address: String,
    label: String,
    expected_fingerprint: Option<String>,
    expected_node_id: Option<Uuid>,
}

impl Target {
    fn manual(address: String) -> Self {
        Self {
            label: address.clone(),
            address,
            expected_fingerprint: None,
            expected_node_id: None,
        }
    }

    fn nearby(candidate: EnrollmentCandidateView) -> Self {
        Self {
            label: friendly_device_name(&candidate.display_name)
                .unwrap_or_else(|| candidate.address.clone()),
            address: candidate.address,
            expected_fingerprint: candidate.advertised_fingerprint,
            expected_node_id: candidate.advertised_node_id,
        }
    }
}

/// Text entry modes. While one is open every key goes to it, so digits do not
/// switch sections and `q` does not quit.
enum Entry {
    Address,
    Code(Target),
}

/// What the footer should advertise while typing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Typing {
    Address,
    Code,
}

#[derive(Default)]
pub struct DevicesUi {
    entry: Option<Entry>,
    candidate: usize,
    trusted: usize,
    revoke: Option<String>,
}

fn plain_char(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(ch)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Some(ch)
        }
        _ => None,
    }
}

impl DevicesUi {
    pub fn typing(&self) -> Option<Typing> {
        match self.entry {
            Some(Entry::Address) => Some(Typing::Address),
            Some(Entry::Code(_)) => Some(Typing::Code),
            None => None,
        }
    }

    fn key_address(&mut self, key: KeyEvent, client: &mut Controller) {
        let mut address = client.view.device_address.clone();
        match key.code {
            KeyCode::Esc => self.entry = None,
            KeyCode::Enter if !address.trim().is_empty() => {
                client.dispatch(Intent::PairingCode(String::new()));
                self.entry = Some(Entry::Code(Target::manual(address.trim().to_owned())));
            }
            KeyCode::Backspace => {
                address.pop();
            }
            _ => {
                if let Some(ch) = plain_char(&key) {
                    address.push(ch);
                }
            }
        }
        client.dispatch(Intent::DeviceAddress(address));
    }

    fn key_code(&mut self, key: KeyEvent, target: Target, client: &mut Controller) {
        let mut code = client.view.pairing_code.as_str().to_owned();
        match key.code {
            KeyCode::Esc => {
                client.dispatch(Intent::PairingCode(String::new()));
                return;
            }
            KeyCode::Enter if client.view.pairing_code.is_complete() => {
                client.dispatch(Intent::Pair {
                    address: target.address.clone(),
                    expected_fingerprint: target.expected_fingerprint.clone(),
                    expected_node_id: target.expected_node_id,
                });
                // Sending clears the code; if the service was busy, keep typing.
                if client.view.pairing_code.is_complete() {
                    self.entry = Some(Entry::Code(target));
                }
                return;
            }
            KeyCode::Backspace => {
                code.pop();
            }
            _ => {
                if let Some(ch) = plain_char(&key).filter(char::is_ascii_digit) {
                    code.push(ch);
                }
            }
        }
        client.dispatch(Intent::PairingCode(code));
        self.entry = Some(Entry::Code(target));
    }

    pub fn key(&mut self, key: KeyEvent, client: &mut Controller) -> bool {
        match self.entry.take() {
            Some(Entry::Address) => {
                self.entry = Some(Entry::Address);
                self.key_address(key, client);
                return true;
            }
            Some(Entry::Code(target)) => {
                self.key_code(key, target, client);
                return true;
            }
            None => {}
        }
        if let Some(fingerprint) = self.revoke.take() {
            if key.code == KeyCode::Char('y') {
                client.dispatch(Intent::RevokeDevice(fingerprint));
            }
            return true;
        }
        let pairing = client.view.pairing_supported;
        match key.code {
            KeyCode::Char('e') => client.dispatch(Intent::OpenEnrollment),
            KeyCode::Char('f') => client.dispatch(Intent::DiscoverEnrollment),
            KeyCode::Char('m') if pairing => {
                self.entry = Some(Entry::Address);
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
                    .filter(|_| pairing)
                {
                    client.dispatch(Intent::PairingCode(String::new()));
                    self.entry = Some(Entry::Code(Target::nearby(candidate)));
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
                Span::raw(" stop"),
            ]),
            Line::from(quiet(
                "To pair, press e on one computer and type the code it shows on the other.",
            )),
            Line::from(""),
        ];
        if !view.enrollment_supported {
            lines.push(Line::styled(
                "Update the service to add devices here.",
                theme.tone(Tone::Warn),
            ));
        } else if !view.pairing_supported {
            lines.push(Line::styled(
                "▲ Update the background service to pair with a code.",
                theme.tone(Tone::Warn),
            ));
        }
        let snapshot = view.enrollment.as_ref();
        match snapshot.and_then(|s| s.code.as_ref()) {
            Some(code) => {
                let accent = theme.accent().add_modifier(Modifier::BOLD);
                lines.push(Line::from(vec![
                    heading("Pairing code "),
                    Span::styled(code.code.grouped(), accent),
                    Span::raw(" · type it on the other computer · "),
                    strong(countdown(code.remaining_secs)),
                ]));
                for row in big_digits(code.code.digits()) {
                    lines.push(Line::from(Span::styled(format!("  {row}"), accent)));
                }
                let tries = if code.attempts_left == 1 {
                    "Last code for this window".to_owned()
                } else {
                    format!("{} codes left in this window", code.attempts_left)
                };
                lines.push(Line::from(quiet(&format!(
                    "Each code works once; a new one appears after a try or when time runs out. {tries}."
                ))));
                if let Some(address) = snapshot.and_then(|s| {
                    s.listen_hints
                        .first()
                        .cloned()
                        .or_else(|| s.listen_addr.clone())
                }) {
                    lines.push(Line::from(vec![
                        quiet("If it is not listed there, use address "),
                        strong(address),
                    ]));
                }
            }
            None if snapshot.is_some_and(|s| s.listening) => {
                lines.push(Line::styled(
                    "Checking a code someone typed…",
                    theme.tone(Tone::Info),
                ));
            }
            None => {}
        }
        match &self.entry {
            Some(Entry::Address) => {
                lines.push(Line::from(vec![
                    quiet("Address of the other computer: "),
                    strong(view.device_address.clone()),
                    Span::styled("▏", theme.accent()),
                ]));
                lines.push(Line::from(vec![
                    key("Enter"),
                    quiet(" next · "),
                    key("Esc"),
                    quiet(" cancel"),
                ]));
            }
            Some(Entry::Code(target)) => {
                let typed = view.pairing_code.as_str();
                let mut boxes = String::new();
                for index in 0..PairingCode::DIGITS {
                    if index == 3 {
                        boxes.push_str("  ");
                    }
                    boxes.push(typed.chars().nth(index).unwrap_or('_'));
                    boxes.push(' ');
                }
                lines.push(Line::from(vec![
                    quiet("Code shown on "),
                    strong(target.label.clone()),
                    quiet(": "),
                    Span::styled(boxes, theme.accent().add_modifier(Modifier::BOLD)),
                ]));
                lines.push(Line::from(vec![
                    key("Enter"),
                    quiet(" pair · "),
                    key("Esc"),
                    quiet(" cancel"),
                ]));
            }
            None => {}
        }
        if let Some(snapshot) = snapshot {
            if let Some(session) = snapshot
                .sessions
                .iter()
                .find(|s| Some(s.session_id) == view.selected_session)
            {
                let who = session
                    .peer_display_name
                    .as_deref()
                    .and_then(friendly_device_name)
                    .or_else(|| session.dialed_addr.clone())
                    .unwrap_or_else(|| "another computer".into());
                let tone = match session.state.as_str() {
                    "Succeeded" => Tone::Good,
                    "Failed" | "Expired" => Tone::Bad,
                    _ => Tone::Info,
                };
                let live = !matches!(
                    session.state.as_str(),
                    "Succeeded" | "Failed" | "Cancelled" | "Expired"
                );
                let mut row = vec![
                    Span::styled(
                        enrollment_state_label(&session.state).to_owned(),
                        theme.tone(tone),
                    ),
                    quiet(&format!(" · {who}")),
                ];
                if snapshot.sessions.len() > 1 {
                    row.extend([quiet(" · "), key("[ / ]"), quiet(" other attempts")]);
                }
                if live {
                    row.extend([quiet(" · "), key("c"), quiet(" cancel")]);
                }
                lines.push(Line::from(row));
                if let Some(error) = &session.error {
                    lines.push(Line::styled(format!("✗ {error}"), theme.tone(Tone::Bad)));
                }
            }
            if let Some(outcome) = &snapshot.outcome {
                let tone = match outcome.status.as_str() {
                    "succeeded" => Tone::Good,
                    "cancelled" => Tone::Quiet,
                    "waiting_peer" => Tone::Info,
                    _ => Tone::Warn,
                };
                let icon = if tone == Tone::Good { "✓ " } else { "" };
                lines.push(Line::styled(
                    format!("{icon}{}", outcome.detail.as_deref().unwrap_or_default()),
                    theme.tone(tone),
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
                    quiet(" type its code"),
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
                .and_then(friendly_device_name)
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
