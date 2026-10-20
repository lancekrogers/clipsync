//! Allowlisted diagnostics for control operations only.
//! The server overlays its in-memory ring at export. This is not a host log:
//! no shell scraping, paths, peer names, secrets, or raw errors.
use crate::control::{
    error_codes,
    schema::{
        ControlSnapshot, DiagnosticError, DiagnosticsReport, DIAGNOSTICS_ERROR_MAX,
        PROTOCOL_VERSION,
    },
};
use chrono::Utc;

pub const REDACTION_LABEL: &str = "clipsync-diagnostics-v1";

pub fn build_report(snapshot: &ControlSnapshot) -> DiagnosticsReport {
    let status = &snapshot.status;
    DiagnosticsReport {
        generated_at: Utc::now().to_rfc3339(),
        app_version: crate::VERSION.to_string(),
        control_version: PROTOCOL_VERSION,
        features: snapshot.features.clone(),
        paused: status.paused,
        connected_peer_count: status.connected_peer_count,
        history_available: status.history_available,
        history_capacity: status.history_capacity,
        max_clipboard_bytes: status.max_clipboard_bytes,
        // D02 will refine; do not invent service inspection here.
        service_mode_hint: "unknown".into(),
        config_labels: allowlisted_config_labels(status),
        // Empty until [`overlay_recent_errors`] replaces this from the server ring.
        recent_errors: Vec::new(),
        redaction: REDACTION_LABEL.into(),
    }
}

/// One ring entry. Unknown codes collapse to the generic control failure.
pub fn diagnostic_error(code: &str) -> DiagnosticError {
    let code = error_codes::canonical_diagnostic_code(code);
    DiagnosticError {
        at: Utc::now().to_rfc3339(),
        code: code.to_string(),
        message: error_codes::fixed_diagnostic_message(code).to_string(),
    }
}

/// Replace `recent_errors` with the server ring. Target-supplied rows are dropped.
pub fn overlay_recent_errors(report: &mut DiagnosticsReport, recorded: Vec<DiagnosticError>) {
    let start = recorded.len().saturating_sub(DIAGNOSTICS_ERROR_MAX);
    report.recent_errors = recorded
        .into_iter()
        .skip(start)
        .map(sanitize_error)
        .collect();
}

fn sanitize_error(entry: DiagnosticError) -> DiagnosticError {
    let code = error_codes::canonical_diagnostic_code(&entry.code);
    DiagnosticError {
        at: safe_timestamp(&entry.at),
        code: code.to_string(),
        message: error_codes::fixed_diagnostic_message(code).to_string(),
    }
}

fn safe_timestamp(at: &str) -> String {
    let ok = (20..=40).contains(&at.len())
        && at
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '-' | ':' | 'T' | 'Z' | '.' | '+'));
    if ok {
        at.to_string()
    } else {
        "1970-01-01T00:00:00Z".to_string()
    }
}

fn allowlisted_config_labels(status: &crate::control::schema::DaemonStatus) -> Vec<String> {
    vec![
        format!("history_capacity={}", status.history_capacity),
        format!("max_clipboard_bytes={}", status.max_clipboard_bytes),
        format!(
            "history_key={}",
            if status.history_available {
                "configured"
            } else {
                "unavailable"
            }
        ),
        format!(
            "pause_feature={}",
            if status.paused {
                error_codes::FEATURE_PAUSE
            } else {
                "running"
            }
        ),
    ]
}
