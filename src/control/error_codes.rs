//! Stable machine-readable control error codes (additive `error_code` on Response).
//! Keep legacy `error` text with the contract's English prefixes for humans.

pub const UNSUPPORTED_PROTOCOL: &str = "unsupported_protocol";
pub const CLIPBOARD_TOO_LARGE: &str = "clipboard_too_large";
pub const FRAME_TOO_LARGE: &str = "frame_too_large";
pub const PEER_OTHER_USER: &str = "peer_other_user";
pub const SYNC_PAUSED: &str = "sync_paused";
pub const HISTORY_UNAVAILABLE: &str = "history_unavailable";
pub const HISTORY_NOT_FOUND: &str = "history_not_found";
pub const HISTORY_QUERY_TOO_LONG: &str = "history_query_too_long";
pub const HISTORY_ENTRY_TOO_LARGE: &str = "history_entry_too_large";
pub const UNSUPPORTED_COMMAND: &str = "unsupported_command";
pub const RESUME_FAILED: &str = "resume_failed";
pub const DAEMON_PAUSED: &str = "daemon_paused";

pub const ENROLLMENT_CLOSED: &str = "enrollment_closed";
pub const ENROLLMENT_EXPIRED: &str = "enrollment_expired";
pub const ENROLLMENT_CANCELLED: &str = "enrollment_cancelled";
pub const ENROLLMENT_RATE_LIMITED: &str = "enrollment_rate_limited";
pub const ENROLLMENT_MISMATCH: &str = "enrollment_mismatch";
pub const ENROLLMENT_REFLECTION: &str = "enrollment_reflection";
pub const ENROLLMENT_REPLAY: &str = "enrollment_replay";
pub const ENROLLMENT_OVERSIZE: &str = "enrollment_oversize";
pub const ENROLLMENT_BUSY: &str = "enrollment_busy";
pub const ENROLLMENT_NOT_FOUND: &str = "enrollment_not_found";
pub const ENROLLMENT_INCOMPLETE: &str = "enrollment_incomplete";
pub const ENROLLMENT_UNAUTHORIZED: &str = "enrollment_unauthorized";

/// Contract feature strings companions require after Status.
pub const FEATURE_PAUSE: &str = "pause";
pub const FEATURE_HISTORY_READ: &str = "history_read";
pub const FEATURE_HISTORY_COPY: &str = "history_copy";
pub const FEATURE_DIAGNOSTICS: &str = "diagnostics";
pub const FEATURE_STATUS_V2: &str = "status_v2";
pub const FEATURE_ENROLLMENT: &str = "enrollment";

pub fn engine_features() -> Vec<String> {
    vec![
        FEATURE_PAUSE.into(),
        FEATURE_HISTORY_READ.into(),
        FEATURE_HISTORY_COPY.into(),
        FEATURE_DIAGNOSTICS.into(),
        FEATURE_STATUS_V2.into(),
    ]
}
