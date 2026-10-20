//! Injected host command runner for service lifecycle operations.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::error::ServiceError;

/// Default wall-clock bound for a single host subprocess.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Default captured stdout/stderr budget per invocation.
pub const DEFAULT_MAX_OUTPUT: usize = 64 * 1024;

/// Bound for draining reader threads after the child exits or is killed.
/// Descendants that inherit pipes must not hang joins indefinitely.
const READER_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Specification for a single host subprocess.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
    pub max_stdout: usize,
    pub max_stderr: usize,
}

impl CommandSpec {
    pub fn new(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            env: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
            max_stdout: DEFAULT_MAX_OUTPUT,
            max_stderr: DEFAULT_MAX_OUTPUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Bounded output from a host subprocess.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        !self.timed_out && self.status == 0
    }

    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Platform command runner. Production uses [`StdHostCommand`]; tests inject fakes.
pub trait HostCommand: Send + Sync {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ServiceError>;
}

/// Production runner wrapping `std::process::Command` with timeout and output bounds.
#[derive(Debug, Default, Clone, Copy)]
pub struct StdHostCommand;

impl HostCommand for StdHostCommand {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ServiceError> {
        run_bounded(spec)
    }
}

fn run_bounded(spec: &CommandSpec) -> Result<CommandOutput, ServiceError> {
    if spec.program.as_os_str().is_empty() {
        return Err(ServiceError::PathInvalid {
            path: PathBuf::new(),
            reason: "host program path is empty".into(),
        });
    }

    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &spec.env {
        command.env(key, value);
    }

    // New process group so timeout can kill descendants that inherit pipes.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let mut child = command
        .spawn()
        .map_err(|err| ServiceError::HostCommandFailed {
            program: spec.program.display().to_string(),
            args: spec.args.clone(),
            status: None,
            stderr: err.to_string(),
            timed_out: false,
        })?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let max_stdout = spec.max_stdout;
    let max_stderr = spec.max_stderr;
    let (stdout_tx, stdout_rx) = mpsc::channel();
    let (stderr_tx, stderr_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = stdout_tx.send(read_bounded(stdout, max_stdout));
    });
    thread::spawn(move || {
        let _ = stderr_tx.send(read_bounded(stderr, max_stderr));
    });

    let outcome = wait_with_timeout(&mut child, spec.timeout)?;
    let timed_out = matches!(outcome, WaitOutcome::TimedOut);
    if timed_out {
        kill_child_tree(&mut child);
        let _ = child.wait();
    }

    let stdout = recv_bounded(stdout_rx, READER_JOIN_TIMEOUT);
    let stderr = recv_bounded(stderr_rx, READER_JOIN_TIMEOUT);
    match outcome {
        WaitOutcome::Exited(status) => Ok(CommandOutput {
            status: status.code().unwrap_or(-1),
            stdout,
            stderr,
            timed_out: false,
        }),
        WaitOutcome::TimedOut => Ok(CommandOutput {
            status: -1,
            stdout,
            stderr,
            timed_out: true,
        }),
    }
}

fn recv_bounded(rx: mpsc::Receiver<Vec<u8>>, limit: Duration) -> Vec<u8> {
    match rx.recv_timeout(limit) {
        Ok(bytes) => bytes,
        Err(_) => Vec::new(),
    }
}

fn kill_child_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // Negative pid: signal the process group started via setpgid.
        unsafe {
            let _ = libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

enum WaitOutcome {
    Exited(std::process::ExitStatus),
    TimedOut,
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Result<WaitOutcome, ServiceError> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(WaitOutcome::Exited(status)),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    return Ok(WaitOutcome::TimedOut);
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(err) => {
                return Err(ServiceError::HostCommandFailed {
                    program: "child".into(),
                    args: Vec::new(),
                    status: None,
                    stderr: err.to_string(),
                    timed_out: false,
                });
            }
        }
    }
}

fn read_bounded(stream: Option<impl Read>, max: usize) -> Vec<u8> {
    let Some(mut stream) = stream else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let remaining = max.saturating_sub(buf.len());
                if remaining == 0 {
                    // Keep draining so the writer is not blocked forever; discard excess.
                    continue;
                }
                let take = n.min(remaining);
                buf.extend_from_slice(&chunk[..take]);
            }
            Err(_) => break,
        }
    }
    buf
}

/// Require a successful host invocation; map failures to [`ServiceError`].
pub fn require_success(
    spec: &CommandSpec,
    output: CommandOutput,
) -> Result<CommandOutput, ServiceError> {
    if output.timed_out {
        return Err(ServiceError::TimedOut {
            program: spec.program.display().to_string(),
            args: spec.args.clone(),
        });
    }
    if output.status != 0 {
        return Err(ServiceError::HostCommandFailed {
            program: spec.program.display().to_string(),
            args: spec.args.clone(),
            status: Some(output.status),
            stderr: output.stderr_lossy(),
            timed_out: false,
        });
    }
    Ok(output)
}

/// Resolve a well-known tool name for PATH lookup (`systemctl`, `launchctl`).
pub fn tool_program(name: &str) -> PathBuf {
    PathBuf::from(name)
}

/// True when `path` is absolute and contains no `..` components.
pub fn is_safe_absolute(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    !path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct ScriptedHost {
        calls: Arc<Mutex<Vec<CommandSpec>>>,
        responses: Arc<Mutex<Vec<Result<CommandOutput, ServiceError>>>>,
    }

    impl HostCommand for ScriptedHost {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ServiceError> {
            self.calls.lock().unwrap().push(spec.clone());
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Ok(CommandOutput {
                    status: 0,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    timed_out: false,
                })
            } else {
                responses.remove(0)
            }
        }
    }

    #[test]
    fn scripted_host_records_calls() {
        let host = ScriptedHost::default();
        let spec = CommandSpec::new("systemctl", ["--user", "is-active", "clipsync.service"]);
        let out = host.run(&spec).unwrap();
        assert!(out.success());
        assert_eq!(host.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn reject_parent_dir_components() {
        assert!(!is_safe_absolute(Path::new("/tmp/../etc/passwd")));
        assert!(is_safe_absolute(Path::new("/tmp/clipsync/config.toml")));
    }
}
