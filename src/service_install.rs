//! Systemd user-service unit generation and inspection (Linux desktop installs).

use std::path::PathBuf;

/// Characters that require systemd quoting for an executable path in `ExecStart=`.
fn path_requires_systemd_quoting(path: &str) -> bool {
    path.chars().any(|c| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\\'
                    | '$'
                    | '%'
                    | '&'
                    | '|'
                    | ';'
                    | '<'
                    | '>'
                    | '*'
                    | '?'
                    | '['
                    | ']'
                    | '('
                    | ')'
                    | '#'
                    | '\''
                    | '`'
                    | '\t'
                    | '\n'
                    | '\r'
            )
    })
}

/// Validate a filesystem path before embedding it in a unit file.
pub fn validate_executable_path(path: &str) -> Result<(), String> {
    if !std::path::Path::new(path).is_absolute() {
        return Err("executable path must be absolute".to_string());
    }
    if path.chars().any(char::is_control) {
        return Err("executable path contains unsupported control characters".to_string());
    }
    // systemd 252 rejects these characters even after unit-file unquoting.
    if path.contains(['\'', '"', '\\']) {
        return Err("systemd executable paths cannot contain quotes or backslashes; choose another install directory".to_string());
    }
    Ok(())
}

/// Quote a binary path for use in a systemd `ExecStart=` line.
///
/// The executable token keeps literal `$`; systemd specifiers still require `%%`.
pub fn systemd_quote_exec_path(path: &str) -> Result<String, String> {
    validate_executable_path(path)?;

    if !path_requires_systemd_quoting(path) {
        return Ok(path.to_string());
    }

    let mut out = String::from('"');
    for ch in path.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    Ok(out)
}

/// Render the `ExecStart=` value for the per-user desktop unit.
pub fn systemd_user_exec_start(binary: &str) -> Result<String, String> {
    let quoted = systemd_quote_exec_path(binary)?;
    Ok(format!("{} start --foreground", quoted))
}

const UNIT_BODY: &str = r#"[Unit]
Description=ClipSync - Cross-platform clipboard synchronization
Documentation=https://github.com/lancekrogers/clipsync
After=graphical-session-pre.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart={exec_start}
Restart=on-failure
RestartSec=10
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=graphical-session.target
"#;

/// Render the canonical per-user systemd unit for the given installed binary.
pub fn render_systemd_user_unit(binary: &str) -> Result<String, String> {
    let exec_start = systemd_user_exec_start(binary)?;
    Ok(UNIT_BODY.replace("{exec_start}", &exec_start))
}

/// Parsed metadata from an on-disk clipsync systemd unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemdUnitInspection {
    pub exec_binary: Option<String>,
    pub uses_foreground: bool,
    pub wanted_by_graphical_session: bool,
    pub orders_after_graphical_session_pre: bool,
}

/// Inspect a clipsync user or system unit file for common installation mistakes.
pub fn inspect_systemd_unit(content: &str) -> SystemdUnitInspection {
    let mut exec_binary = None;
    let mut uses_foreground = false;
    let mut wanted_by_graphical_session = false;
    let mut orders_after_graphical_session_pre = false;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("ExecStart=") {
            let exec = line.trim_start_matches("ExecStart=").trim();
            uses_foreground =
                exec.contains("start --foreground") || exec.contains("start\t--foreground");
            exec_binary = parse_exec_start_binary(exec);
        } else if line.starts_with("WantedBy=") {
            wanted_by_graphical_session = line.contains("graphical-session.target");
        } else if line.starts_with("After=") {
            orders_after_graphical_session_pre = line.contains("graphical-session-pre.target");
        }
    }

    SystemdUnitInspection {
        exec_binary,
        uses_foreground,
        wanted_by_graphical_session,
        orders_after_graphical_session_pre,
    }
}

fn parse_exec_start_binary(exec_start: &str) -> Option<String> {
    let trimmed = exec_start.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.starts_with('"') {
        let mut out = String::new();
        let mut escaped = false;
        for ch in trimmed.chars().skip(1) {
            if escaped {
                out.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                break;
            } else {
                out.push(ch);
            }
        }
        return Some(out.replace("%%", "%"));
    }

    trimmed.split_whitespace().next().map(|s| s.to_string())
}

/// Default per-user install location used by supported installers.
pub fn default_user_binary_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".local").join("bin").join("clipsync"))
}

/// Known locations that may indicate duplicate legacy/system installs.
pub fn legacy_system_unit_paths() -> &'static [&'static str] {
    &[
        "/etc/systemd/system/clipsync.service",
        "/lib/systemd/system/clipsync.service",
        "/usr/lib/systemd/system/clipsync.service",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_quote_handles_spaces() {
        assert_eq!(
            systemd_quote_exec_path("/home/me/My Apps/clipsync").unwrap(),
            "\"/home/me/My Apps/clipsync\""
        );
    }

    #[test]
    fn systemd_quote_preserves_dollar_and_escapes_percent() {
        assert_eq!(
            systemd_quote_exec_path("/opt/$USER/%n/bin").unwrap(),
            "\"/opt/$USER/%%n/bin\""
        );
    }

    #[test]
    fn systemd_quote_escapes_ampersand_and_pipe() {
        let quoted = systemd_quote_exec_path("/opt/a&b|c").unwrap();
        assert_eq!(quoted, "\"/opt/a&b|c\"");
    }

    #[test]
    fn render_unit_matches_foreground_graphical_session() {
        let unit = render_systemd_user_unit("/home/me/.local/bin/clipsync").unwrap();
        let inspection = inspect_systemd_unit(&unit);
        assert_eq!(
            inspection.exec_binary.as_deref(),
            Some("/home/me/.local/bin/clipsync")
        );
        assert!(inspection.uses_foreground);
        assert!(inspection.wanted_by_graphical_session);
        assert!(inspection.orders_after_graphical_session_pre);
        assert!(!unit.contains("Wants=graphical-session.target"));
    }
}
