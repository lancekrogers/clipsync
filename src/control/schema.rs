//! Version-1 control wire DTOs (D04 / CONTROL_CONTRACT + D03 enrollment IPC).
use crate::adapters::Peer;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u32 = 1;
pub const HISTORY_PAGE_MAX: u32 = 20;
pub const HISTORY_PREVIEW_CHARS: usize = 240;
pub const HISTORY_QUERY_MAX_BYTES: usize = 256;
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
    DiagnosticsExport,
    EnrollmentOpen,
    EnrollmentDiscover,
    EnrollmentDial {
        address: String,
        expected_fingerprint: Option<String>,
        expected_node_id: Option<Uuid>,
    },
    EnrollmentInspect {
        session_id: Option<Uuid>,
    },
    EnrollmentConfirm {
        session_id: Uuid,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentSnapshot {
    pub listening: bool,
    pub listen_addr: Option<String>,
    pub expires_at: Option<String>,
    pub sessions: Vec<EnrollmentSessionView>,
    pub candidates: Vec<EnrollmentCandidateView>,
    pub outcome: Option<EnrollmentOutcomeView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentSessionView {
    pub session_id: Uuid,
    pub role: String,
    pub state: String,
    pub peer_display_name: Option<String>,
    pub local_fingerprint: String,
    pub peer_fingerprint: Option<String>,
    pub peer_node_id: Option<Uuid>,
    pub dialed_addr: Option<String>,
    pub local_confirmed: bool,
    pub peer_confirmed: bool,
    pub remaining_secs: u64,
    pub error: Option<String>,
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
