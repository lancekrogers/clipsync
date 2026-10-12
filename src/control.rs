#![deny(warnings, clippy::all)]
//! Per-user daemon control. The socket directory and peer credentials protect both ends.
use crate::{
    adapters::Peer,
    config::Config,
    sync::{SyncEngine, TrustAwareSyncEngine},
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    time::timeout,
};

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
    Copy { text: String },
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub version: u32,
    pub pid: u32,
    pub peers: Vec<Peer>,
    pub queued: Option<usize>,
    pub error: Option<String>,
}
#[async_trait::async_trait]
pub trait Target: Send + Sync {
    async fn peers(&self) -> Vec<Peer>;
    async fn sync(&self) -> Result<usize>;
    async fn copy_text(&self, text: String) -> Result<usize>;
}
#[async_trait::async_trait]
impl Target for SyncEngine {
    async fn peers(&self) -> Vec<Peer> {
        self.get_connected_peers().await
    }
    async fn sync(&self) -> Result<usize> {
        self.force_sync().await
    }
    async fn copy_text(&self, text: String) -> Result<usize> {
        self.copy_local(text).await
    }
}
#[async_trait::async_trait]
impl Target for TrustAwareSyncEngine {
    async fn peers(&self) -> Vec<Peer> {
        self.get_connected_peers().await
    }
    async fn sync(&self) -> Result<usize> {
        self.force_sync().await
    }
    async fn copy_text(&self, text: String) -> Result<usize> {
        self.copy_local(text).await
    }
}
/// Worst-case JSON string encoding per UTF-8 byte (`\u00XX`).
const JSON_UTF8_BYTE_EXPANSION: usize = 6;
/// Copy request fields outside the string payload (version, keys, braces).
const COPY_REQUEST_FIXED_OVERHEAD: usize = 128;
/// Status/peers/sync responses and non-copy requests.
const RESPONSE_FRAME_HEADROOM: usize = 512;
const MIN_CONTROL_FRAME: usize = 8192;

pub fn max_frame_bytes(config: &Config) -> usize {
    let max_clipboard = config.clipboard.max_size;
    let copy_request = COPY_REQUEST_FIXED_OVERHEAD
        .saturating_add(max_clipboard.saturating_mul(JSON_UTF8_BYTE_EXPANSION));
    copy_request
        .max(RESPONSE_FRAME_HEADROOM)
        .max(MIN_CONTROL_FRAME)
}
pub fn socket_path(config: &Config) -> PathBuf {
    use sha2::{Digest, Sha256};
    let identity = format!(
        "{}\n{}",
        config.auth.authorized_keys.display(),
        config.clipboard.history_db.display()
    );
    let hash = hex::encode(Sha256::digest(identity.as_bytes()));
    // User-runtime sockets remain visible when a systemd service uses PrivateTmp.
    #[cfg(target_os = "linux")]
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            let runtime = PathBuf::from(format!("/run/user/{}", unsafe { libc::geteuid() }));
            runtime.is_dir().then_some(runtime)
        })
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(not(target_os = "linux"))]
    let base = std::env::temp_dir();
    base.join(format!("clipsync-{}", unsafe { libc::geteuid() }))
        .join(format!("{}.sock", &hash[..20]))
}
fn private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
    {
        bail!("Control directory must be owned by this user with mode 0700");
    }
    Ok(())
}
fn check_peer(stream: &UnixStream) -> Result<()> {
    if stream.peer_cred()?.uid() != unsafe { libc::geteuid() } {
        bail!("Control peer belongs to another user");
    }
    Ok(())
}
pub struct Server {
    listener: UnixListener,
    path: PathBuf,
    _lock: fs::File,
    max_frame: usize,
    max_clipboard: usize,
}
impl Server {
    pub fn bind(path: PathBuf, config: &Config) -> Result<Self> {
        let max_frame = max_frame_bytes(config);
        let max_clipboard = config.clipboard.max_size;
        private_directory(path.parent().context("Socket has no parent")?)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path.with_extension("lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("ClipSync daemon already owns this control socket");
        }
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            listener,
            path,
            _lock: lock,
            max_frame,
            max_clipboard,
        })
    }
    pub async fn run(&self, target: &dyn Target) -> Result<()> {
        loop {
            let (mut stream, _) = self.listener.accept().await?;
            let result = timeout(Duration::from_secs(3), async {
                check_peer(&stream)?;
                let request: Request = read_json(&mut stream, self.max_frame).await?;
                let mut response = Response {
                    version: 1,
                    pid: std::process::id(),
                    peers: Vec::new(),
                    queued: None,
                    error: None,
                };
                if request.version != 1 {
                    response.error = Some("Unsupported control protocol".into());
                } else {
                    match request.command {
                        Command::Sync => match target.sync().await {
                            Ok(n) => response.queued = Some(n),
                            Err(e) => response.error = Some(e.to_string()),
                        },
                        Command::Copy { text } => {
                            if text.len() > self.max_clipboard {
                                response.error =
                                    Some("Clipboard exceeds configured size limit".into());
                            } else {
                                match target.copy_text(text).await {
                                    Ok(n) => response.queued = Some(n),
                                    Err(e) => response.error = Some(e.to_string()),
                                }
                            }
                        }
                        Command::Status | Command::Peers => response.peers = target.peers().await,
                    }
                }
                write_json(&mut stream, &response, self.max_frame).await
            })
            .await;
            if let Ok(Err(e)) = result {
                tracing::debug!("Control request rejected: {e}");
            }
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
async fn read_json<T: serde::de::DeserializeOwned>(
    stream: &mut UnixStream,
    max_frame: usize,
) -> Result<T> {
    let length = stream.read_u32().await? as usize;
    if length > max_frame {
        bail!("Control frame exceeds {max_frame} bytes");
    }
    let mut data = vec![0; length];
    stream.read_exact(&mut data).await?;
    Ok(serde_json::from_slice(&data)?)
}
async fn write_json<T: Serialize>(
    stream: &mut UnixStream,
    value: &T,
    max_frame: usize,
) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    if data.len() > max_frame {
        bail!("Control frame exceeds {max_frame} bytes");
    }
    stream.write_u32(data.len() as u32).await?;
    stream.write_all(&data).await?;
    Ok(())
}
fn daemon_unreachable_message() -> String {
    #[cfg(target_os = "linux")]
    {
        "ClipSync daemon is not reachable. Start it with `clipsync start --foreground` using the same `--config` path.".into()
    }
    #[cfg(not(target_os = "linux"))]
    {
        "ClipSync daemon is not reachable".into()
    }
}

pub async fn request(path: &Path, command: Command, max_frame: usize) -> Result<Response> {
    timeout(Duration::from_secs(4), async {
        let mut stream = UnixStream::connect(path)
            .await
            .context(daemon_unreachable_message())?;
        check_peer(&stream)?;
        write_json(
            &mut stream,
            &Request {
                version: 1,
                command,
            },
            max_frame,
        )
        .await?;
        let response: Response = read_json(&mut stream, max_frame).await?;
        if response.version != 1 {
            bail!("Unsupported daemon control protocol");
        }
        if let Some(error) = &response.error {
            bail!("{error}");
        }
        Ok(response)
    })
    .await
    .context("Daemon control request timed out")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_frame_fits_worst_case_copy_request() {
        let mut config = Config::default();
        config.clipboard.max_size = 16 * 1024;
        let text = "\u{1}".repeat(config.clipboard.max_size);
        let frame = serde_json::to_vec(&Request {
            version: 1,
            command: Command::Copy { text },
        })
        .unwrap();
        assert!(frame.len() <= max_frame_bytes(&config));
    }
}
