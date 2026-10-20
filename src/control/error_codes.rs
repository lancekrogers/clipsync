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

/// Ring-only. Handler failures that have no contract `error_code`.
pub const CONTROL_FAILED: &str = "control_failed";
/// Ring-only. The 3s accept handler deadline fired.
pub const CONTROL_TIMEOUT: &str = "control_timeout";
/// Ring-only. The frame was not a version-1 request, or the response write failed.
pub const CONTROL_PROTOCOL: &str = "control_protocol";

const GENERIC_DIAGNOSTIC_MESSAGE: &str = "The control operation failed.";

/// Allowlisted diagnostic code. Anything else becomes [`CONTROL_FAILED`].
pub fn canonical_diagnostic_code(code: &str) -> &'static str {
    diagnostic_entry(code)
        .map(|(stable, _)| stable)
        .unwrap_or(CONTROL_FAILED)
}

/// Fixed ring text. Never the raw handler or parser string.
pub fn fixed_diagnostic_message(code: &str) -> &'static str {
    diagnostic_entry(code)
        .map(|(_, message)| message)
        .unwrap_or(GENERIC_DIAGNOSTIC_MESSAGE)
}

fn diagnostic_entry(code: &str) -> Option<(&'static str, &'static str)> {
    const ENTRIES: &[(&str, &str)] = &[
        (
            UNSUPPORTED_PROTOCOL,
            "The control protocol version is not supported.",
        ),
        (
            CLIPBOARD_TOO_LARGE,
            "The clipboard payload exceeds the configured limit.",
        ),
        (
            FRAME_TOO_LARGE,
            "The control request exceeds the frame limit.",
        ),
        (
            PEER_OTHER_USER,
            "The control connection belongs to another user.",
        ),
        (SYNC_PAUSED, "Sync is paused."),
        (HISTORY_UNAVAILABLE, "History is unavailable."),
        (HISTORY_NOT_FOUND, "The history entry was not found."),
        (HISTORY_QUERY_TOO_LONG, "The history query is too long."),
        (
            HISTORY_ENTRY_TOO_LARGE,
            "The history entry exceeds the configured limit.",
        ),
        (UNSUPPORTED_COMMAND, "The control command is not supported."),
        (RESUME_FAILED, "Resume failed and sync stays paused."),
        (
            DAEMON_PAUSED,
            "Sync is paused. Copies stay on this computer.",
        ),
        (ENROLLMENT_CLOSED, "Enrollment is not open."),
        (ENROLLMENT_EXPIRED, "Enrollment verification expired."),
        (ENROLLMENT_CANCELLED, "Enrollment was cancelled."),
        (ENROLLMENT_RATE_LIMITED, "Enrollment is rate limited."),
        (
            ENROLLMENT_MISMATCH,
            "Enrollment verification did not match.",
        ),
        (ENROLLMENT_REFLECTION, "Enrollment was refused."),
        (ENROLLMENT_REPLAY, "Enrollment was refused."),
        (
            ENROLLMENT_OVERSIZE,
            "Enrollment was refused because the message was too large.",
        ),
        (ENROLLMENT_BUSY, "Enrollment is already in progress."),
        (
            ENROLLMENT_NOT_FOUND,
            "The enrollment session was not found.",
        ),
        (ENROLLMENT_INCOMPLETE, "Enrollment is not finished."),
        (ENROLLMENT_UNAUTHORIZED, "Enrollment was not authorized."),
        (CONTROL_TIMEOUT, "The control request timed out."),
        (CONTROL_PROTOCOL, "The control request was rejected."),
        (CONTROL_FAILED, GENERIC_DIAGNOSTIC_MESSAGE),
    ];
    ENTRIES
        .iter()
        .find(|(candidate, _)| *candidate == code)
        .copied()
}

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
