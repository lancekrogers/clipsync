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
}
#[async_trait::async_trait]
impl Target for SyncEngine {
    async fn peers(&self) -> Vec<Peer> {
        self.get_connected_peers().await
    }
    async fn sync(&self) -> Result<usize> {
        self.force_sync().await
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
}
impl Server {
    pub fn bind(path: PathBuf) -> Result<Self> {
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
        })
    }
    pub async fn run(&self, target: &dyn Target) -> Result<()> {
        loop {
            let (mut stream, _) = self.listener.accept().await?;
            let result = timeout(Duration::from_secs(3), async {
                check_peer(&stream)?;
                let request: Request = read_json(&mut stream).await?;
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
                        Command::Status | Command::Peers => response.peers = target.peers().await,
                    }
                }
                write_json(&mut stream, &response).await
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
async fn read_json<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let length = stream.read_u32().await? as usize;
    if length > 8192 {
        bail!("Control frame exceeds 8192 bytes");
    }
    let mut data = vec![0; length];
    stream.read_exact(&mut data).await?;
    Ok(serde_json::from_slice(&data)?)
}
async fn write_json<T: Serialize>(stream: &mut UnixStream, value: &T) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    if data.len() > 8192 {
        bail!("Control frame exceeds 8192 bytes");
    }
    stream.write_u32(data.len() as u32).await?;
    stream.write_all(&data).await?;
    Ok(())
}
pub async fn request(path: &Path, command: Command) -> Result<Response> {
    timeout(Duration::from_secs(4), async {
        let mut stream = UnixStream::connect(path)
            .await
            .context("ClipSync daemon is not reachable")?;
        check_peer(&stream)?;
        write_json(
            &mut stream,
            &Request {
                version: 1,
                command,
            },
        )
        .await?;
        let response: Response = read_json(&mut stream).await?;
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
