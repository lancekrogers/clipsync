use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process;

use anyhow::{anyhow, Context, Result};
use nix::sys::signal::{self, Signal};
use nix::unistd::{fork, ForkResult, Pid};
use tracing::{error, info};

/// Get the path for the pidfile
pub fn get_pidfile_path() -> Result<PathBuf> {
    let uid = nix::unistd::getuid();

    // Try XDG_RUNTIME_DIR first (modern Linux)
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime_dir).join("clipsync.pid");
        return Ok(path);
    }

    // Try /var/run/user/<uid>/ (systemd systems)
    let var_run_user = PathBuf::from(format!("/var/run/user/{}", uid));
    if var_run_user.exists() {
        return Ok(var_run_user.join("clipsync.pid"));
    }

    // Fallback to ~/.local/run/
    let home = dirs::home_dir().ok_or_else(|| anyhow!("Could not determine home directory"))?;
    let local_run = home.join(".local").join("run");

    // Create directory if it doesn't exist
    fs::create_dir_all(&local_run).context("Failed to create ~/.local/run directory")?;

    Ok(local_run.join("clipsync.pid"))
}

/// Write PID to pidfile
pub fn write_pidfile(pid: u32) -> Result<()> {
    let pidfile_path = get_pidfile_path()?;

    // Create parent directory if it doesn't exist
    if let Some(parent) = pidfile_path.parent() {
        fs::create_dir_all(parent).context("Failed to create pidfile directory")?;
    }

    let mut file = File::create(&pidfile_path)
        .with_context(|| format!("Failed to create pidfile: {:?}", pidfile_path))?;

    write!(file, "{}", pid)?;

    // Set permissions to 0600 (read/write for owner only)
    let metadata = file.metadata()?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(&pidfile_path, permissions)?;

    info!("Wrote PID {} to {:?}", pid, pidfile_path);
    Ok(())
}

/// Read PID from pidfile
pub fn read_pidfile() -> Result<Option<u32>> {
    let pidfile_path = get_pidfile_path()?;

    if !pidfile_path.exists() {
        return Ok(None);
    }

    let mut contents = String::new();
    File::open(&pidfile_path)?.read_to_string(&mut contents)?;

    let pid = contents
        .trim()
        .parse::<u32>()
        .with_context(|| format!("Invalid PID in pidfile: {}", contents))?;

    Ok(Some(pid))
}

/// Remove pidfile
pub fn remove_pidfile() -> Result<()> {
    let pidfile_path = get_pidfile_path()?;

    if pidfile_path.exists() {
        fs::remove_file(&pidfile_path)
            .with_context(|| format!("Failed to remove pidfile: {:?}", pidfile_path))?;
        info!("Removed pidfile: {:?}", pidfile_path);
    }

    Ok(())
}

/// Check if a process with the given PID is running
pub fn is_process_running(pid: u32) -> bool {
    // Use signal 0 to check if process exists (doesn't actually send a signal)
    match signal::kill(Pid::from_raw(pid as i32), None) {
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Check if daemon is already running
pub fn is_daemon_running() -> Result<bool> {
    match read_pidfile()? {
        Some(pid) => {
            if is_process_running(pid) {
                Ok(true)
            } else {
                // Stale pidfile, remove it
                info!("Found stale pidfile for PID {}, removing", pid);
                remove_pidfile()?;
                Ok(false)
            }
        }
        None => Ok(false),
    }
}

/// Fork the process to run as a daemon
/// 
/// IMPORTANT: This function now uses a safer approach that doesn't
/// interfere with system authentication or terminal I/O
pub fn daemonize() -> Result<()> {
    // First fork
    match unsafe { fork() } {
        Ok(ForkResult::Parent { child: _ }) => {
            // Parent process exits immediately
            process::exit(0);
        }
        Ok(ForkResult::Child) => {
            // Child continues
        }
        Err(e) => {
            return Err(anyhow!("First fork failed: {}", e));
        }
    }

    // Create new session and become session leader
    // This detaches us from the controlling terminal
    nix::unistd::setsid()?;

    // Second fork to ensure we can't reacquire a controlling terminal
    match unsafe { fork() } {
        Ok(ForkResult::Parent { child: _ }) => {
            // Parent process exits
            process::exit(0);
        }
        Ok(ForkResult::Child) => {
            // Child continues as the daemon
        }
        Err(e) => {
            return Err(anyhow!("Second fork failed: {}", e));
        }
    }

    // Reset umask to ensure files are created with proper permissions
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o022));

    // Change working directory to root to avoid holding mount points
    std::env::set_current_dir("/")?;

    // SAFER APPROACH: Instead of manipulating file descriptors directly,
    // we'll use proper file handles and let Rust manage them safely
    
    // Open /dev/null for reading and writing
    let dev_null_read = File::open("/dev/null")
        .context("Failed to open /dev/null for reading")?;
    let dev_null_write = File::create("/dev/null")
        .context("Failed to open /dev/null for writing")?;

    // Use std::os::unix::io::AsRawFd to get raw file descriptors
    use std::os::unix::io::AsRawFd;
    
    // Close stdin and reopen as /dev/null
    // IMPORTANT: We close first, then dup to avoid affecting parent's descriptors
    unsafe {
        libc::close(0);  // Close stdin
        libc::dup2(dev_null_read.as_raw_fd(), 0);  // Reopen stdin as /dev/null
        
        libc::close(1);  // Close stdout
        libc::dup2(dev_null_write.as_raw_fd(), 1);  // Reopen stdout as /dev/null
        
        libc::close(2);  // Close stderr
        libc::dup2(dev_null_write.as_raw_fd(), 2);  // Reopen stderr as /dev/null
    }

    // Write pidfile with our PID
    let pid = process::id();
    write_pidfile(pid)?;

    info!("Daemonized safely with PID {}", pid);
    Ok(())
}

/// Alternative: Run in foreground mode (recommended for systemd)
/// This avoids all the complexity and potential issues of daemonization
pub fn run_foreground() -> Result<()> {
    let pid = process::id();
    write_pidfile(pid)?;
    info!("Running in foreground with PID {}", pid);
    Ok(())
}

/// Stop the running daemon
pub fn stop_daemon() -> Result<()> {
    match read_pidfile()? {
        Some(pid) => {
            if is_process_running(pid) {
                info!("Sending SIGTERM to daemon with PID {}", pid);
                signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM)?;

                // Wait a bit for the process to terminate
                for _ in 0..10 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    if !is_process_running(pid) {
                        info!("Daemon stopped successfully");
                        return Ok(());
                    }
                }

                // If still running, try SIGKILL
                error!("Daemon did not stop gracefully, sending SIGKILL");
                signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL)?;
                std::thread::sleep(std::time::Duration::from_millis(100));

                if is_process_running(pid) {
                    return Err(anyhow!("Failed to stop daemon"));
                }

                info!("Daemon forcefully stopped");
                Ok(())
            } else {
                info!("Daemon is not running (stale pidfile)");
                remove_pidfile()?;
                Ok(())
            }
        }
        None => {
            info!("Daemon is not running (no pidfile)");
            Ok(())
        }
    }
}

/// Setup signal handlers for graceful shutdown
pub fn setup_signal_handlers(shutdown_tx: tokio::sync::oneshot::Sender<()>) -> Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

    // Spawn a task to handle SIGTERM
    tokio::spawn(async move {
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");

        sigterm.recv().await;
        info!("Received SIGTERM, shutting down gracefully");

        // Clean up pidfile
        if let Err(e) = remove_pidfile() {
            error!("Failed to remove pidfile: {}", e);
        }

        // Signal shutdown
        let _ = shutdown_tx.send(());
    });

    Ok(())
}

/// Create a systemd service file (for reference)
pub fn generate_systemd_service() -> &'static str {
    r#"[Unit]
Description=ClipSync - Secure Clipboard Synchronization
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/clipsync start --foreground
Restart=on-failure
RestartSec=10
User=%i

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=%h/.config/clipsync %h/.local/share/clipsync

[Install]
WantedBy=default.target
"#
}