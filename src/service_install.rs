//! Systemd user-service unit generation and inspection (Linux desktop installs).
//!
//! Config-aware rendering lives here for CLI/`print-user-unit` compatibility.
//! Lifecycle operations (start/stop/enable) are in [`crate::service`].

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

/// Whether an argument token needs double quotes (after `$`/`%` are escaped in-place).
fn arg_requires_systemd_quoting(arg: &str) -> bool {
    arg.chars().any(|c| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\\'
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

/// Validate a config path before embedding it as an `ExecStart` argument.
pub fn validate_config_path(path: &str) -> Result<(), String> {
    if !std::path::Path::new(path).is_absolute() {
        return Err("config path must be absolute".to_string());
    }
    if path.chars().any(char::is_control) {
        return Err("config path contains unsupported control characters".to_string());
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

/// Quote a non-executable `ExecStart` argument (e.g. `--config` value).
///
/// Unlike [`systemd_quote_exec_path`], argument tokens escape `$` as `$$` and
/// `%` as `%%` whether or not the token is double-quoted. `$`/`%` alone do not
/// force surrounding quotes.
pub fn systemd_quote_arg(arg: &str) -> Result<String, String> {
    if arg.chars().any(char::is_control) {
        return Err("argument contains unsupported control characters".to_string());
    }

    let needs_quote = arg_requires_systemd_quoting(arg);
    let mut out = String::with_capacity(arg.len() + 2);
    if needs_quote {
        out.push('"');
    }
    for ch in arg.chars() {
        match ch {
            '$' => out.push_str("$$"),
            '%' => out.push_str("%%"),
            '\\' if needs_quote => out.push_str("\\\\"),
            '"' if needs_quote => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    if needs_quote {
        out.push('"');
    }
    Ok(out)
}

/// Render the `ExecStart=` value for the per-user desktop unit (legacy, no `--config`).
pub fn systemd_user_exec_start(binary: &str) -> Result<String, String> {
    let quoted = systemd_quote_exec_path(binary)?;
    Ok(format!("{quoted} start --foreground"))
}

/// Render `ExecStart=` with an explicit absolute `--config` argument.
pub fn systemd_user_exec_start_with_config(binary: &str, config: &str) -> Result<String, String> {
    validate_config_path(config)?;
    let quoted_bin = systemd_quote_exec_path(binary)?;
    let quoted_config = systemd_quote_arg(config)?;
    Ok(format!(
        "{quoted_bin} --config {quoted_config} start --foreground"
    ))
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
///
/// Compatibility wrapper: omits `--config` (pre-D02 shape). Prefer
/// [`render_systemd_user_unit_with_config`] for new installs.
pub fn render_systemd_user_unit(binary: &str) -> Result<String, String> {
    render_systemd_user_unit_with_config(binary, None)
}

/// Render the per-user systemd unit, optionally embedding `--config <path>`.
pub fn render_systemd_user_unit_with_config(
    binary: &str,
    config: Option<&str>,
) -> Result<String, String> {
    let exec_start = match config {
        Some(path) => systemd_user_exec_start_with_config(binary, path)?,
        None => systemd_user_exec_start(binary)?,
    };
    Ok(UNIT_BODY.replace("{exec_start}", &exec_start))
}

/// Parsed metadata from an on-disk clipsync systemd unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemdUnitInspection {
    pub exec_binary: Option<String>,
    pub config_path: Option<String>,
    pub uses_foreground: bool,
    pub wanted_by_graphical_session: bool,
    pub orders_after_graphical_session_pre: bool,
}

/// Inspect a clipsync user or system unit file for common installation mistakes.
pub fn inspect_systemd_unit(content: &str) -> SystemdUnitInspection {
    let mut exec_binary = None;
    let mut config_path = None;
    let mut uses_foreground = false;
    let mut wanted_by_graphical_session = false;
    let mut orders_after_graphical_session_pre = false;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("ExecStart=") {
            let exec = line.trim_start_matches("ExecStart=").trim();
            uses_foreground =
                exec.contains("start --foreground") || exec.contains("start\t--foreground");
            let tokens = tokenize_exec_start(exec);
            if let Some(bin) = tokens.first() {
                exec_binary = Some(bin.clone());
            }
            config_path = extract_config_arg(&tokens);
        } else if line.starts_with("WantedBy=") {
            wanted_by_graphical_session = line.contains("graphical-session.target");
        } else if line.starts_with("After=") {
            orders_after_graphical_session_pre = line.contains("graphical-session-pre.target");
        }
    }

    SystemdUnitInspection {
        exec_binary,
        config_path,
        uses_foreground,
        wanted_by_graphical_session,
        orders_after_graphical_session_pre,
    }
}

fn extract_config_arg(tokens: &[String]) -> Option<String> {
    let mut iter = tokens.iter();
    while let Some(tok) = iter.next() {
        if tok == "--config" {
            return iter.next().cloned();
        }
        if let Some(rest) = tok.strip_prefix("--config=") {
            return Some(rest.to_string());
        }
    }
    None
}

/// Split an `ExecStart=` value into tokens with systemd-style unquoting.
fn tokenize_exec_start(exec_start: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = exec_start.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        if ch == '"' {
            chars.next();
            let mut out = String::new();
            let mut escaped = false;
            for ch in chars.by_ref() {
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
            tokens.push(unescape_systemd_arg(&out));
        } else {
            let mut out = String::new();
            while let Some(&ch) = chars.peek() {
                if ch.is_whitespace() {
                    break;
                }
                out.push(ch);
                chars.next();
            }
            tokens.push(unescape_systemd_arg(&out));
        }
    }
    tokens
}

fn unescape_systemd_arg(raw: &str) -> String {
    // Argument / path tokens: collapse %% → % and $$ → $.
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if (ch == '%' || ch == '$') && chars.peek() == Some(&ch) {
            chars.next();
            out.push(ch);
        } else {
            out.push(ch);
        }
    }
    out
}

fn parse_exec_start_binary(exec_start: &str) -> Option<String> {
    tokenize_exec_start(exec_start).into_iter().next()
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

/// True when any known legacy system unit file exists on disk (inspect-only).
pub fn legacy_system_unit_present() -> bool {
    legacy_system_unit_paths()
        .iter()
        .any(|path| std::path::Path::new(path).is_file())
}

/// Escape a string for inclusion in a macOS plist XML `<string>` element.
pub fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() && c != '\t' && c != '\n' && c != '\r' => {
                // Drop other controls rather than emit illegal XML text.
            }
            c => out.push(c),
        }
    }
    out
}

/// Render the managed LaunchAgent plist with absolute binary and config paths.
pub fn render_launch_agent_plist(
    binary: &str,
    config: &str,
    stdout_path: &str,
    stderr_path: &str,
    working_directory: &str,
    path_env: &str,
) -> Result<String, String> {
    if !std::path::Path::new(binary).is_absolute() {
        return Err("binary path must be absolute".into());
    }
    validate_config_path(config)?;
    if binary.chars().any(char::is_control) || config.chars().any(char::is_control) {
        return Err("paths contain unsupported control characters".into());
    }

    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.clipsync</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>--config</string>
        <string>{config}</string>
        <string>start</string>
        <string>--foreground</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{stdout}</string>
    <key>StandardErrorPath</key>
    <string>{stderr}</string>
    <key>WorkingDirectory</key>
    <string>{workdir}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>{path_env}</string>
    </dict>
</dict>
</plist>
"#,
        binary = xml_escape(binary),
        config = xml_escape(config),
        stdout = xml_escape(stdout_path),
        stderr = xml_escape(stderr_path),
        workdir = xml_escape(working_directory),
        path_env = xml_escape(path_env),
    ))
}

/// Parsed LaunchAgent metadata.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchAgentInspection {
    pub label: Option<String>,
    pub program_arguments: Vec<String>,
    pub exec_binary: Option<String>,
    pub config_path: Option<String>,
    pub run_at_load: bool,
    pub keep_alive: bool,
}

/// Best-effort parse of a ClipSync LaunchAgent plist (string-array focused).
pub fn inspect_launch_agent_plist(content: &str) -> LaunchAgentInspection {
    let mut inspection = LaunchAgentInspection::default();
    let mut in_program_args = false;
    let mut expecting_label = false;
    let mut expecting_run_at_load = false;
    let mut expecting_keep_alive = false;

    for raw in content.lines() {
        let line = raw.trim();
        if line == "<key>Label</key>" {
            expecting_label = true;
            continue;
        }
        if expecting_label {
            if let Some(value) = extract_xml_string(line) {
                inspection.label = Some(value);
            }
            expecting_label = false;
            continue;
        }
        if line == "<key>ProgramArguments</key>" {
            in_program_args = true;
            continue;
        }
        if in_program_args {
            if line == "</array>" {
                in_program_args = false;
                continue;
            }
            if let Some(value) = extract_xml_string(line) {
                inspection.program_arguments.push(value);
            }
            continue;
        }
        if line == "<key>RunAtLoad</key>" {
            expecting_run_at_load = true;
            continue;
        }
        if expecting_run_at_load {
            inspection.run_at_load = line == "<true/>" || line == "<true />";
            expecting_run_at_load = false;
            continue;
        }
        if line == "<key>KeepAlive</key>" {
            expecting_keep_alive = true;
            continue;
        }
        if expecting_keep_alive {
            inspection.keep_alive = line == "<true/>" || line == "<true />";
            expecting_keep_alive = false;
            continue;
        }
    }

    if let Some(bin) = inspection.program_arguments.first() {
        inspection.exec_binary = Some(bin.clone());
    }
    inspection.config_path = extract_config_arg(&inspection.program_arguments);
    inspection
}

fn extract_xml_string(line: &str) -> Option<String> {
    let start = line.find("<string>")? + "<string>".len();
    let end = line.find("</string>")?;
    if end < start {
        return None;
    }
    Some(xml_unescape(&line[start..end]))
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
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
    fn systemd_arg_escapes_dollar_and_percent_independently() {
        assert_eq!(
            systemd_quote_arg("/home/$USER/%n/config.toml").unwrap(),
            "/home/$$USER/%%n/config.toml"
        );
        assert_eq!(
            systemd_quote_arg("/home/me/My Configs/config.toml").unwrap(),
            "\"/home/me/My Configs/config.toml\""
        );
        assert_eq!(
            systemd_quote_arg("/cfg/$x/%y with space.toml").unwrap(),
            "\"/cfg/$$x/%%y with space.toml\""
        );
    }

    #[test]
    fn render_unit_with_config_roundtrips_inspection() {
        let unit = render_systemd_user_unit_with_config(
            "/home/me/.local/bin/clipsync",
            Some("/home/me/.config/clipsync/config.toml"),
        )
        .unwrap();
        let inspection = inspect_systemd_unit(&unit);
        assert_eq!(
            inspection.exec_binary.as_deref(),
            Some("/home/me/.local/bin/clipsync")
        );
        assert_eq!(
            inspection.config_path.as_deref(),
            Some("/home/me/.config/clipsync/config.toml")
        );
        assert!(inspection.uses_foreground);
        assert!(inspection.wanted_by_graphical_session);
        assert!(inspection.orders_after_graphical_session_pre);
    }

    #[test]
    fn render_unit_compat_omits_config() {
        let unit = render_systemd_user_unit("/home/me/.local/bin/clipsync").unwrap();
        let inspection = inspect_systemd_unit(&unit);
        assert!(inspection.config_path.is_none());
        assert_eq!(
            inspection.exec_binary.as_deref(),
            Some("/home/me/.local/bin/clipsync")
        );
        assert!(!unit.contains("Wants=graphical-session.target"));
    }

    #[test]
    fn launch_agent_xml_escapes_and_parses() {
        let plist = render_launch_agent_plist(
            "/Apps/Clip & Sync/clipsync",
            "/Users/a/<cfg>/config.toml",
            "/tmp/out",
            "/tmp/err",
            "/Users/a",
            "/Apps/Clip & Sync:/usr/bin",
        )
        .unwrap();
        assert!(plist.contains("Clip &amp; Sync"));
        assert!(plist.contains("&lt;cfg&gt;"));
        let inspection = inspect_launch_agent_plist(&plist);
        assert_eq!(inspection.label.as_deref(), Some("com.clipsync"));
        assert_eq!(
            inspection.exec_binary.as_deref(),
            Some("/Apps/Clip & Sync/clipsync")
        );
        assert_eq!(
            inspection.config_path.as_deref(),
            Some("/Users/a/<cfg>/config.toml")
        );
        assert!(inspection.run_at_load);
        assert!(inspection.keep_alive);
    }

    #[test]
    fn parse_exec_start_binary_still_works() {
        assert_eq!(
            parse_exec_start_binary(
                "\"/opt/a b/clipsync\" --config /c/config.toml start --foreground"
            ),
            Some("/opt/a b/clipsync".into())
        );
    }
}
