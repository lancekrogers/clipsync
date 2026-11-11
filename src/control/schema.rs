//! Version-1 control wire DTOs, including device pairing.
use crate::adapters::Peer;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u32 = 1;
pub const HISTORY_PAGE_MAX: u32 = 20;
pub const HISTORY_PREVIEW_CHARS: usize = 240;
pub const HISTORY_QUERY_MAX_BYTES: usize = 256;
/// Thumbnail dimensions keep the JSON response below the minimum frame budget.
pub const IMAGE_PREVIEW_MAX_DIMENSION: u32 = 64;
pub const DIAGNOSTICS_ERROR_MAX: usize = 20;
/// Minimum frame budget reserved for a worst-case 20-preview history page.
pub const HISTORY_PAGE_FRAME_MIN: usize = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub command: Command,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Command {
    Status,
    Peers,
    Sync,
    Copy {
        text: String,
    },
    SetPause {
        paused: bool,
    },
    HistoryList {
        limit: u32,
    },
    HistorySearch {
        query: String,
        limit: u32,
    },
    HistoryGet {
        id: Uuid,
    },
    HistoryCopy {
        id: Uuid,
    },
    /// Requires feature `history_delete`; older daemons cannot parse it.
    HistoryDelete {
        id: Uuid,
    },
    /// Requires feature `history_clear`; older daemons cannot parse it.
    HistoryClear,
    DiagnosticsExport,
    EnrollmentOpen,
    EnrollmentDiscover,
    /// Requires feature `enrollment_code`. Dials `address` and proves the code
    /// shown there; nothing else is confirmed by hand.
    EnrollmentPair {
        address: String,
        code: PairingCode,
        expected_fingerprint: Option<String>,
        expected_node_id: Option<Uuid>,
    },
    EnrollmentInspect {
        session_id: Option<Uuid>,
    },
    EnrollmentCancel {
        session_id: Option<Uuid>,
    },
    EnrollmentRevoke {
        fingerprint: String,
    },
    TrustedDevices,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub version: u32,
    pub pid: u32,
    pub peers: Vec<Peer>,
    pub queued: Option<usize>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<DaemonStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryPage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_entry: Option<HistoryEntryDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<DiagnosticsReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_applied: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enrollment: Option<EnrollmentSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_devices: Option<Vec<TrustedDeviceView>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_removal: Option<HistoryRemovalView>,
}

impl Response {
    pub fn base() -> Self {
        Self {
            version: PROTOCOL_VERSION,
            pid: std::process::id(),
            peers: Vec::new(),
            queued: None,
            error: None,
            features: None,
            instance_id: None,
            state_generation: None,
            status: None,
            history: None,
            history_entry: None,
            diagnostics: None,
            pause_applied: None,
            error_code: None,
            enrollment: None,
            trusted_devices: None,
            history_removal: None,
        }
    }

    pub fn fail(&mut self, code: &str, message: impl Into<String>) {
        self.error_code = Some(code.to_string());
        self.error = Some(message.into());
    }
}

/// Typed control failure preserving stable `error_code` for clients.
#[derive(Debug, Clone)]
pub struct ControlError {
    pub code: Option<String>,
    pub message: String,
}

impl ControlError {
    pub fn from_response(response: &Response) -> Option<Self> {
        response.error.as_ref().map(|message| Self {
            code: response.error_code.clone(),
            message: message.clone(),
        })
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(code) = &self.code {
            write!(f, "{code}: {}", self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for ControlError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub paused: bool,
    pub observed_at: String,
    pub connected_peer_count: u32,
    pub history_available: bool,
    pub node_id: String,
    pub max_clipboard_bytes: usize,
    pub history_capacity: u32,
    pub outbound_pending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryPage {
    pub query: Option<String>,
    pub total_returned: u32,
    pub truncated: bool,
    pub entries: Vec<HistoryEntryView>,
}

/// Result of HistoryDelete / HistoryClear on this daemon.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryRemovalView {
    /// Rows removed locally; 0 for a delete whose id was already gone.
    pub removed: u32,
    /// Whether this device's system clipboard was cleared.
    pub clipboard_cleared: bool,
    /// Connected peers the removal was queued to. Queued, not acknowledged.
    pub peers_notified: u32,
    /// Connected peers not sent the removal (older version or full queue).
    pub peers_skipped: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntryView {
    pub id: Uuid,
    pub timestamp: String,
    pub origin_node: Uuid,
    pub content_type: String,
    pub content_chars: u32,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntryDetail {
    pub id: Uuid,
    pub timestamp: String,
    pub origin_node: Uuid,
    pub content_type: String,
    pub content_chars: u32,
    pub preview: String,
    pub text: String,
    /// Small RGB thumbnail; absent for text, old daemons, or invalid images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_preview: Option<ImagePreview>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImagePreview {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    /// Optional, bounded high-resolution PNG for terminals with graphics support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub png: Option<String>,
}

impl ImagePreview {
    pub fn is_valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.width <= IMAGE_PREVIEW_MAX_DIMENSION
            && self.height <= IMAGE_PREVIEW_MAX_DIMENSION
            && self.rgb.len() == self.width as usize * self.height as usize * 3
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsReport {
    pub generated_at: String,
    pub app_version: String,
    pub control_version: u32,
    pub features: Vec<String>,
    pub paused: bool,
    pub connected_peer_count: u32,
    pub history_available: bool,
    pub history_capacity: u32,
    pub max_clipboard_bytes: usize,
    pub service_mode_hint: String,
    pub config_labels: Vec<String>,
    pub recent_errors: Vec<DiagnosticError>,
    pub redaction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticError {
    pub at: String,
    pub code: String,
    pub message: String,
}

/// A six-digit pairing code. `Debug` never shows the digits, so a snapshot or
/// command can be logged without leaking it; only serialization to the local
/// UI carries them.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PairingCode(String);

impl PairingCode {
    pub const DIGITS: usize = 6;

    /// Exactly six ASCII digits; spaces between groups are ignored.
    pub fn parse(text: &str) -> Option<Self> {
        let digits: String = text.chars().filter(|ch| *ch != ' ').collect();
        (digits.len() == Self::DIGITS && digits.bytes().all(|b| b.is_ascii_digit()))
            .then_some(Self(digits))
    }

    pub fn digits(&self) -> &str {
        &self.0
    }

    /// `482 913`: two groups of three for reading aloud.
    pub fn grouped(&self) -> String {
        format!("{} {}", &self.0[..3], &self.0[3..])
    }
}

impl TryFrom<String> for PairingCode {
    type Error = &'static str;
    fn try_from(mut text: String) -> Result<Self, Self::Error> {
        let parsed = Self::parse(&text).ok_or("pairing code must be six digits");
        zeroize::Zeroize::zeroize(&mut text);
        parsed
    }
}

impl From<PairingCode> for String {
    fn from(mut code: PairingCode) -> Self {
        std::mem::take(&mut code.0)
    }
}

impl std::fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingCode(<redacted>)")
    }
}

impl Drop for PairingCode {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

/// The live code on the allowing computer. Sent only to the local UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingCodeView {
    pub code: PairingCode,
    pub remaining_secs: u64,
    /// Pairing attempts this window still accepts; each attempt uses one code.
    pub attempts_left: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentSnapshot {
    /// Session created by this dial request. Ordinary polls do not change focus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_session_id: Option<Uuid>,
    pub listening: bool,
    pub listen_addr: Option<String>,
    /// Addresses to type on the other computer when it cannot find this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub listen_hints: Vec<String>,
    pub expires_at: Option<String>,
    pub sessions: Vec<EnrollmentSessionView>,
    pub candidates: Vec<EnrollmentCandidateView>,
    pub outcome: Option<EnrollmentOutcomeView>,
    /// Present while this computer allows a device and a code is live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<PairingCodeView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentSessionView {
    pub session_id: Uuid,
    /// `initiator` typed a code and dialed; `acceptor` showed the code.
    pub role: String,
    pub state: String,
    pub peer_display_name: Option<String>,
    pub local_fingerprint: String,
    pub peer_fingerprint: Option<String>,
    pub peer_node_id: Option<Uuid>,
    pub dialed_addr: Option<String>,
    pub remaining_secs: u64,
    pub error: Option<String>,
    /// Stable reason for `Failed` and incomplete sessions (see [`PairingFailure`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

/// Why a pairing attempt stopped. The string forms are stable wire values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingFailure {
    /// Nothing accepted the connection: the other computer is not allowing a device.
    NotListening,
    /// The address could not be reached at all.
    Unreachable,
    /// Something answered, but not ClipSync pairing.
    NotPairing,
    /// The other computer runs a ClipSync without code pairing.
    UpdateRequired,
    /// The other computer is allowing a device but has no live code right now.
    NoCode,
    /// The code was wrong, expired or already used.
    WrongCode,
    /// The address belongs to this computer.
    Reflection,
    /// The device answering is not the one that was picked from the nearby list.
    Mismatch,
    Busy,
    RateLimited,
    /// The other side broke the protocol.
    Protocol,
    Disconnected,
    /// This computer could not save the authorization.
    SaveFailed,
    /// Saved on this computer; the other one did not confirm it saved.
    Incomplete,
    Cancelled,
    Expired,
}

impl PairingFailure {
    pub const ALL: [Self; 16] = [
        Self::NotListening,
        Self::Unreachable,
        Self::NotPairing,
        Self::UpdateRequired,
        Self::NoCode,
        Self::WrongCode,
        Self::Reflection,
        Self::Mismatch,
        Self::Busy,
        Self::RateLimited,
        Self::Protocol,
        Self::Disconnected,
        Self::SaveFailed,
        Self::Incomplete,
        Self::Cancelled,
        Self::Expired,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotListening => "not_listening",
            Self::Unreachable => "unreachable",
            Self::NotPairing => "not_pairing",
            Self::UpdateRequired => "update_required",
            Self::NoCode => "no_code",
            Self::WrongCode => "wrong_code",
            Self::Reflection => "reflection",
            Self::Mismatch => "mismatch",
            Self::Busy => "busy",
            Self::RateLimited => "rate_limited",
            Self::Protocol => "protocol",
            Self::Disconnected => "disconnected",
            Self::SaveFailed => "save_failed",
            Self::Incomplete => "incomplete",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.as_str() == text)
    }

    /// Fixed text for the person who typed the code (or showed it).
    pub fn message(self) -> &'static str {
        match self {
            Self::NotListening => "Nothing is accepting pairing at that address. On the other computer, choose Allow a new device, then try again.",
            Self::Unreachable => "That computer could not be reached. Check the address and that both computers are on the same network.",
            Self::NotPairing => "That address is not a ClipSync pairing address. Use the address shown next to the code on the other computer.",
            Self::UpdateRequired => "The other computer runs an older ClipSync. Update ClipSync there, then pair again.",
            Self::NoCode => "The other computer has no pairing code open right now. Choose Allow a new device there for a new code.",
            Self::WrongCode => "That code did not match. Each code works once: type the new code shown on the other computer.",
            Self::Reflection => "That address is this computer. Pair with the other computer's address.",
            Self::Mismatch => "The computer that answered is not the one picked from the nearby list. Nothing was paired.",
            Self::Busy => "The other computer is busy with other pairing attempts. Try again in a moment.",
            Self::RateLimited => "Too many pairing attempts. Wait a few minutes and try again.",
            Self::Protocol => "The other computer sent something unexpected. Nothing was paired.",
            Self::Disconnected => "The connection dropped before pairing finished. Nothing was paired.",
            Self::SaveFailed => "This computer could not save the pairing. Nothing was paired here.",
            Self::Incomplete => "Saved on this computer, but the other computer did not confirm it saved. Remove the device here and pair again.",
            Self::Cancelled => "Pairing was cancelled.",
            Self::Expired => "Pairing took too long and stopped.",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentCandidateView {
    pub display_name: String,
    pub address: String,
    pub advertised_fingerprint: Option<String>,
    pub advertised_node_id: Option<Uuid>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentOutcomeView {
    pub status: String,
    pub peer_fingerprint: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedDeviceView {
    pub fingerprint: String,
    pub comment: Option<String>,
}

/// Engine-facing snapshot used by Target implementations (not always on the wire).
#[derive(Debug, Clone)]
pub struct ControlSnapshot {
    pub peers: Vec<Peer>,
    pub features: Vec<String>,
    pub instance_id: Uuid,
    pub state_generation: u64,
    pub status: DaemonStatus,
}
