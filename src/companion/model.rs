//! Renderer-neutral interaction state. Neither renderer owns clipboard contents or sync.
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Route {
    #[default]
    Overview,
    Devices,
    History,
    Settings,
}
impl Route {
    pub const ALL: [Self; 4] = [Self::Overview, Self::Devices, Self::History, Self::Settings];
    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Devices => "Devices",
            Self::History => "History",
            Self::Settings => "Settings",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    #[default]
    Checking,
    Ready,
    Connected(usize),
    Paused,
    Unavailable,
    UpgradeRequired,
}
impl Connection {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Checking => "Checking service",
            Self::Ready => "Ready to sync",
            Self::Connected(_) => "Connected",
            Self::Paused => "Paused",
            Self::Unavailable => "Service unavailable",
            Self::UpgradeRequired => "Service update required",
        }
    }
}

/// Names and previews may arrive from untrusted peers. Keep terminal controls out
/// of both renderers; preserve Unicode text and deliberate multiline details.
pub fn display_text(text: &str, multiline: bool) -> String {
    text.chars()
        .filter_map(|ch| {
            if ch == '\n' || ch == '\u{2028}' || ch == '\u{2029}' {
                Some(if multiline { '\n' } else { ' ' })
            } else if ch == '\t' {
                Some(' ')
            } else if ch.is_control() || is_unsafe_format(ch) {
                None
            } else {
                Some(ch)
            }
        })
        .collect()
}

/// A short human name from an authorization comment or advertised label.
/// Enrollment stores `<name> SHA256:…` and `clipsync setup` stores
/// `clipsync-<host>`; advertised names default to `<host>-clipsync`.
/// Input must already be sanitized with [`display_text`].
pub fn friendly_device_name(label: &str) -> Option<String> {
    let words = label
        .split_whitespace()
        .filter(|word| !word.starts_with("SHA256:"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut name = words.as_str();
    if name
        .get(..9)
        .is_some_and(|head| head.eq_ignore_ascii_case("clipsync-"))
    {
        name = &name[9..];
    }
    if let Some(cut) = name.len().checked_sub(9) {
        if name
            .get(cut..)
            .is_some_and(|tail| tail.eq_ignore_ascii_case("-clipsync"))
        {
            name = &name[..cut];
        }
    }
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let mut chars = name.chars();
    let short: String = chars.by_ref().take(32).collect();
    Some(if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    })
}

/// Bidirectional and invisible format marks that can reorder a label without
/// being Unicode control characters (`Cc`).
pub(crate) fn is_unsafe_format(ch: char) -> bool {
    matches!(
        ch,
        '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

/// A token is scoped to an input domain so periodic status cannot invalidate a
/// search, and late search/detail replies cannot overwrite newer selections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    Status,
    History,
    Detail,
    Action,
    Setup,
    Enrollment,
    Devices,
}
impl Domain {
    fn index(self) -> usize {
        self as usize
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub domain: Domain,
    pub sequence: u64,
    pub epoch: u64,
}

/// How a snapshot compares with the newest generation already applied for one
/// daemon instance. Restarts are not applied here; Status owns the epoch bump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotOrder {
    Fresh,
    Stale,
    OtherInstance,
}

#[derive(Debug, Default)]
pub struct Reconciliation {
    next: u64,
    latest: [u64; 7],
    epoch: u64,
    instance: Option<Uuid>,
    generation: u64,
    last_status: Option<Instant>,
    failures: u8,
}
impl Reconciliation {
    pub fn instance_id(&self) -> Option<Uuid> {
        self.instance
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Record a completion that is newer for this process. Older generations and
    /// other daemon instances are refused so a late reply cannot roll state back.
    pub fn note_generation(&mut self, instance: Uuid, generation: u64) -> SnapshotOrder {
        if let Some(current) = self.instance {
            if current != instance {
                return SnapshotOrder::OtherInstance;
            }
            if generation < self.generation {
                return SnapshotOrder::Stale;
            }
        }
        self.instance = Some(instance);
        self.generation = generation;
        SnapshotOrder::Fresh
    }
    /// A full status snapshot proves the service is answering.
    pub fn mark_fresh(&mut self, now: Instant) {
        self.last_status = Some(now);
        self.failures = 0;
    }
    pub fn begin(&mut self, domain: Domain) -> Ticket {
        self.next = self.next.wrapping_add(1);
        self.latest[domain.index()] = self.next;
        Ticket {
            domain,
            sequence: self.next,
            epoch: self.epoch,
        }
    }
    pub fn current(&self, ticket: Ticket) -> bool {
        ticket.epoch == self.epoch && self.latest[ticket.domain.index()] == ticket.sequence
    }
    /// Called only for the newest Status reply. Accept a generation reset when
    /// the daemon restarted, then invalidate all other outstanding operations.
    pub fn status(
        &mut self,
        ticket: Ticket,
        instance: Uuid,
        generation: u64,
        now: Instant,
    ) -> bool {
        if !self.current(ticket) {
            return false;
        }
        if self.instance == Some(instance) && generation < self.generation {
            return false;
        }
        if self.instance.is_some() && self.instance != Some(instance) {
            self.epoch = self.epoch.wrapping_add(1);
        }
        self.instance = Some(instance);
        self.generation = generation;
        self.last_status = Some(now);
        self.failures = 0;
        true
    }
    pub fn failed(&mut self, ticket: Ticket) {
        if self.current(ticket) && ticket.domain == Domain::Status {
            self.failures = self.failures.saturating_add(1);
        }
    }
    pub fn stale(&self, now: Instant) -> bool {
        self.failures >= 2
            || self
                .last_status
                .is_none_or(|at| now.saturating_duration_since(at) > Duration::from_secs(5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_and_selection_replies_cannot_replace_newer_input() {
        let mut state = Reconciliation::default();
        let old = state.begin(Domain::History);
        let new = state.begin(Domain::History);
        let status = state.begin(Domain::Status);
        assert!(!state.current(old));
        assert!(state.current(new));
        assert!(state.current(status));
    }
    #[test]
    fn daemon_restart_accepts_generation_reset_and_invalidates_inflight_detail() {
        let mut state = Reconciliation::default();
        let ticket = state.begin(Domain::Status);
        let a = Uuid::new_v4();
        assert!(state.status(ticket, a, 50, Instant::now()));
        let detail = state.begin(Domain::Detail);
        let ticket = state.begin(Domain::Status);
        assert!(state.status(ticket, Uuid::new_v4(), 0, Instant::now()));
        assert!(!state.current(detail));
        assert!(!state.stale(Instant::now()));
    }
    #[test]
    fn failures_and_elapsed_time_mark_state_stale() {
        let mut state = Reconciliation::default();
        let now = Instant::now();
        let ticket = state.begin(Domain::Status);
        assert!(state.status(ticket, Uuid::new_v4(), 0, now));
        assert!(state.stale(now + Duration::from_secs(6)));
        for _ in 0..2 {
            let ticket = state.begin(Domain::Status);
            state.failed(ticket);
        }
        assert!(state.stale(now));
    }
    #[test]
    fn peer_text_cannot_emit_terminal_escape_sequences_or_bidi_overrides() {
        assert_eq!(
            display_text("hello\x1b[2J\r\nworld\u{202e}", false),
            "hello[2J world"
        );
        assert_eq!(display_text("a\nb\t", true), "a\nb ");
        assert_eq!(
            display_text("a\u{2028}b\u{202e}c\u{200f}\u{feff}", false),
            "a bc"
        );
        assert_eq!(display_text("a\u{2028}b\u{2029}c", true), "a\nb\nc");
    }
    #[test]
    fn device_names_drop_fingerprints_and_clipsync_affixes() {
        let fingerprint = "SHA256:TtXaCtU8Moi9iEkJFkmdyuY+R+ViWhCd5e8r2r+/v4c";
        assert_eq!(
            friendly_device_name(&format!("workbench-clipsync {fingerprint}")).as_deref(),
            Some("workbench")
        );
        assert_eq!(
            friendly_device_name("clipsync-laptop").as_deref(),
            Some("laptop")
        );
        assert_eq!(
            friendly_device_name("Lance's MacBook Air").as_deref(),
            Some("Lance's MacBook Air")
        );
        assert_eq!(
            friendly_device_name("clipsync").as_deref(),
            Some("clipsync")
        );
        assert_eq!(friendly_device_name(fingerprint), None);
        assert_eq!(friendly_device_name("  "), None);
        assert_eq!(friendly_device_name("é-clipsync").as_deref(), Some("é"));
        let long = "x".repeat(40);
        assert_eq!(
            friendly_device_name(&long).map(|name| name.chars().count()),
            Some(33)
        );
    }

    #[test]
    fn older_generation_and_other_instances_cannot_replace_a_newer_snapshot() {
        let mut state = Reconciliation::default();
        let ticket = state.begin(Domain::Status);
        let instance = Uuid::new_v4();
        assert!(state.status(ticket, instance, 1, Instant::now()));
        assert_eq!(state.note_generation(instance, 4), SnapshotOrder::Fresh);
        assert_eq!(state.generation(), 4);
        let late = state.begin(Domain::Status);
        assert!(!state.status(late, instance, 2, Instant::now()));
        assert_eq!(state.generation(), 4);
        assert_eq!(
            state.note_generation(Uuid::new_v4(), 0),
            SnapshotOrder::OtherInstance
        );
        assert_eq!(state.instance_id(), Some(instance));
        assert_eq!(state.generation(), 4);
    }
}

/// User-facing labels shared by both renderers; protocol state names stay stable.
pub fn enrollment_state_label(state: &str) -> &str {
    match state {
        "Dialing" => "Connecting…",
        "Verifying" => "Checking the code…",
        "Persisting" => "Saving the pairing…",
        "WaitingPeerAuth" => "Saved here; waiting for the other computer",
        "Succeeded" => "Paired",
        "Cancelled" => "Cancelled",
        "Expired" => "Timed out",
        "Failed" => "Not paired",
        _ => state,
    }
}

/// `1:52` for a countdown in seconds.
pub fn countdown(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The pairing code in three-row block digits, for reading across a room.
pub fn big_digits(digits: &str) -> [String; 3] {
    const FONT: [[&str; 3]; 10] = [
        ["┏━┓", "┃ ┃", "┗━┛"],
        ["╺┓ ", " ┃ ", "╺┻╸"],
        ["╺━┓", "┏━┛", "┗━╸"],
        ["╺━┓", " ━┫", "╺━┛"],
        ["╻ ╻", "┗━┫", "  ╹"],
        ["┏━╸", "┗━┓", "╺━┛"],
        ["┏━╸", "┣━┓", "┗━┛"],
        ["╺━┓", "  ┃", "  ╹"],
        ["┏━┓", "┣━┫", "┗━┛"],
        ["┏━┓", "┗━┫", "╺━┛"],
    ];
    let mut rows: [String; 3] = Default::default();
    for (index, digit) in digits.bytes().filter(u8::is_ascii_digit).enumerate() {
        for (row, line) in rows.iter_mut().enumerate() {
            if index > 0 {
                line.push_str(if index == 3 { "   " } else { " " });
            }
            line.push_str(FONT[usize::from(digit - b'0')][row]);
        }
    }
    rows
}

#[cfg(test)]
mod pairing_tests {
    use super::*;

    #[test]
    fn big_digits_keep_columns_aligned_and_group_by_three() {
        let rows = big_digits("482913");
        let width = rows[0].chars().count();
        assert!(rows.iter().all(|row| row.chars().count() == width));
        assert_eq!(width, 6 * 3 + 4 + 3);
        assert_eq!(countdown(112), "1:52");
        assert_eq!(countdown(5), "0:05");
    }
}
