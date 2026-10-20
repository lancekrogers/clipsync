//! Allowlisted diagnostics only — no payloads, paths, peer labels, or raw errors.
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
        // Allowlisted ring only; never serialize raw anyhow/IO text.
        recent_errors: truncate_errors(Vec::new()),
        redaction: REDACTION_LABEL.into(),
    }
}

fn truncate_errors(mut errors: Vec<DiagnosticError>) -> Vec<DiagnosticError> {
    errors.truncate(DIAGNOSTICS_ERROR_MAX);
    errors
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
