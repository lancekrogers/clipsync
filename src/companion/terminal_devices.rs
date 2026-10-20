//! Keyboard interactions for the same enrollment intents as the desktop.
use super::client::{Controller, Intent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;

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
    pub fn lines(&self, client: &Controller) -> Vec<Line<'static>> {
        let mut lines = vec![
            Line::from("e allow a device · f find nearby · m enter address · x close enrollment"),
            Line::from("Compare BOTH full fingerprints on BOTH computers before confirming."),
            Line::from(""),
        ];
        if !client.view.enrollment_supported {
            lines.push(Line::from("Update the service to add devices here."));
        }
        if self.editing {
            lines.push(Line::from(format!(
                "Address: {} · Enter connect / Esc cancel",
                client.view.device_address
            )));
        }
        if let Some(snapshot) = &client.view.enrollment {
            if snapshot.listening {
                lines.push(Line::from(format!(
                    "Allowing connections: {}",
                    snapshot.listen_addr.as_deref().unwrap_or("opening…")
                )));
            }
            if let Some(session) = snapshot
                .sessions
                .iter()
                .find(|s| Some(s.session_id) == client.view.selected_session)
            {
                lines.extend([
                    Line::from(format!(
                        "{} · {}s left · [ / ] switch session",
                        super::model::enrollment_state_label(&session.state),
                        session.remaining_secs
                    )),
                    Line::from("This computer:"),
                    Line::from(session.local_fingerprint.clone()),
                    Line::from("Other computer:"),
                    Line::from(
                        session
                            .peer_fingerprint
                            .clone()
                            .unwrap_or_else(|| "Waiting for secure connection…".into()),
                    ),
                    Line::from(if session.local_confirmed {
                        "Confirmed here; awaiting both computers. · c cancel"
                    } else {
                        "y fingerprints match, confirm · c cancel"
                    }),
                ]);
                if let Some(error) = &session.error {
                    lines.push(Line::from(error.clone()));
                }
            }
            if let Some(outcome) = &snapshot.outcome {
                lines.push(Line::from(format!(
                    "{}: {}",
                    outcome.status,
                    outcome.detail.as_deref().unwrap_or_default()
                )));
            }
            if !snapshot.candidates.is_empty() {
                lines.push(Line::from("Nearby · ↑/↓ select · Enter connect"));
            }
            for (index, candidate) in snapshot.candidates.iter().enumerate() {
                lines.push(Line::from(format!(
                    "{} {} · {}",
                    if index == self.candidate { ">" } else { " " },
                    candidate.display_name,
                    candidate.address
                )));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from("Connected now"));
        if client.view.devices.is_empty() {
            lines.push(Line::from("No authenticated devices connected."));
        }
        for device in &client.view.devices {
            lines.push(Line::from(format!("{} · {}", device.name, device.address)));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(
            "Authorized devices · j/k select · D remove authorization",
        ));
        for (index, device) in client.view.trusted_devices.iter().enumerate() {
            lines.push(Line::from(format!(
                "{} {}",
                if index == self.trusted { ">" } else { " " },
                device.comment.as_deref().unwrap_or("Device")
            )));
            lines.push(Line::from(device.fingerprint.clone()));
        }
        if let Some(fingerprint) = &self.revoke {
            // Put destructive confirmation first so it remains visible in a small terminal.
            lines.splice(
                0..0,
                [
                    Line::from("REMOVE authorization? y confirms; any other key cancels."),
                    Line::from(fingerprint.clone()),
                ],
            );
        }
        lines
    }
}
