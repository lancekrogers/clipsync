//! One desktop process per chosen configuration.
//!
//! Failure policy:
//! - The private runtime directory must already be, or be created as, mode `0700`
//!   and owned by this uid. A symlink, another owner, or any group/other bit is
//!   rejected and left unchanged.
//! - The lock file is a regular inode opened with `O_NOFOLLOW`, mode `0600`,
//!   owned by this uid, then `File::try_lock`. The lock inode is never removed.
//!   That avoids an unlink race where a successor creates a new lock while the
//!   old owner unlinks the name.
//! - `WouldBlock` means a live owner. This process sends one bounded activation
//!   and exits successfully. It does not construct [`clipsync::companion::app::Companion`]
//!   and does not start a daemon. If the owner cannot be signaled, this process
//!   fails instead of becoming a second owner.
//! - Only the lock holder may unlink a socket, and only when the path is a
//!   non-symlink socket inode owned by this uid. Another owner's socket, a
//!   symlink, or a non-socket is left in place and acquisition fails.
//! - A crash drops the flock and leaves the lock inode and socket behind. The
//!   next same-user owner replaces that owned socket. A killed process cannot
//!   run cleanup; the successor does, under the lock.
//! - Shutdown unlinks the socket only when the path still names the bound inode.
//!   The lock file stays. Same-user peers are inside the trust boundary, matching
//!   daemon control: both ends require a matching uid from `SO_PEERCRED` (Linux)
//!   or `getpeereid` (other Unix). A missing credential fails closed.
//! - The activation record is at most 64 bytes, read with a one-second timeout.
//!   Any other payload does not show the window.

use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use clipsync::config::resolution::{ResolveOutcome, ResolvePaths, resolve_config};

const ACTIVATION: &[u8] = b"v1\n";
const ACTIVATION_OK: &[u8] = b"ok\n";
const MAX_ACTIVATION: usize = 64;
const IO_TIMEOUT: Duration = Duration::from_secs(1);
const SIGNAL_ATTEMPTS: usize = 40;
const SIGNAL_PAUSE: Duration = Duration::from_millis(25);

#[derive(Debug)]
pub struct InstanceError {
    message: String,
}

impl InstanceError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for InstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for InstanceError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SocketDisposition {
    /// Same-uid non-symlink socket. The lock holder may replace it.
    Replace,
    /// Different uid. Do not unlink.
    RefuseOtherOwner,
    /// Symlink or non-socket. Do not unlink.
    RefuseUnsafe,
}

pub(crate) fn socket_disposition(
    is_symlink: bool,
    is_socket: bool,
    owner: u32,
    self_uid: u32,
) -> SocketDisposition {
    if owner != self_uid {
        SocketDisposition::RefuseOtherOwner
    } else if is_symlink || !is_socket {
        SocketDisposition::RefuseUnsafe
    } else {
        SocketDisposition::Replace
    }
}

enum Shared {
    Idle,
    Wake(Box<dyn Fn() + Send + Sync>),
}

struct SharedState {
    pending: AtomicBool,
    waker: Mutex<Shared>,
}

impl SharedState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: AtomicBool::new(false),
            waker: Mutex::new(Shared::Idle),
        })
    }

    fn signal(&self) {
        let slot = self
            .waker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.pending.store(true, Ordering::Release);
        if let Shared::Wake(waker) = &*slot {
            waker();
        }
    }

    fn set_waker(&self, waker: Box<dyn Fn() + Send + Sync>) {
        let mut slot = self
            .waker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Shared::Wake(waker);
        if self.pending.load(Ordering::Acquire)
            && let Shared::Wake(waker) = &*slot
        {
            waker();
        }
    }
}

/// Primary owner. Drop releases the flock and the bound socket name.
pub struct Guard {
    _lock: File,
    socket_path: PathBuf,
    socket_dev: u64,
    socket_ino: u64,
    wake: Option<UnixStream>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<SharedState>,
}

pub enum Launch {
    Primary(Guard),
    /// The existing window was asked to become visible and focused.
    Signaled,
}

impl Guard {
    /// Register the egui wake. Safe to call once the context exists.
    /// A show requested before this still runs the waker.
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        self.shared.set_waker(Box::new(waker));
    }

    /// One queued show/focus request.
    pub fn take_activation(&self) -> bool {
        self.shared.pending.swap(false, Ordering::AcqRel)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(wake) = self.wake.take() {
            let _ = wake.shutdown(std::net::Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // The lock is still held, so a successor cannot have bound yet.
        // Skip the unlink when the name no longer refers to our inode.
        if let Ok(meta) = fs::symlink_metadata(&self.socket_path)
            && !meta.file_type().is_symlink()
            && meta.file_type().is_socket()
            && meta.uid() == current_uid()
            && meta.dev() == self.socket_dev
            && meta.ino() == self.socket_ino
        {
            let _ = fs::remove_file(&self.socket_path);
        }
    }
}

/// Acquire the instance for the desktop process configuration.
/// Call this before `Companion::new`.
pub fn claim(explicit: Option<&Path>) -> Result<Launch, InstanceError> {
    let paths =
        ResolvePaths::from_process_env().map_err(|error| InstanceError::new(error.to_string()))?;
    let material = identity_material(explicit, &paths)?;
    claim_at(&material, &desktop_runtime_dir()?)
}

pub(crate) fn claim_at(material: &[u8], directory: &Path) -> Result<Launch, InstanceError> {
    private_directory(directory)?;
    let (socket_name, lock_name) = artifact_names(material);
    let socket_path = directory.join(socket_name);
    let lock_path = directory.join(lock_name);
    let lock = open_lock(&lock_path)?;
    match lock.try_lock() {
        Ok(()) => bind_primary(lock, socket_path),
        Err(std::fs::TryLockError::WouldBlock) => signal_owner(&socket_path),
        Err(std::fs::TryLockError::Error(error)) => {
            Err(InstanceError::new(format!("instance lock failed: {error}")))
        }
    }
}

fn bind_primary(lock: File, socket_path: PathBuf) -> Result<Launch, InstanceError> {
    replace_stale_socket(&socket_path)?;
    let listener = match UnixListener::bind(&socket_path) {
        Ok(listener) => listener,
        Err(error) => {
            return Err(InstanceError::new(format!(
                "instance socket bind failed: {error}"
            )));
        }
    };
    if let Err(error) = seal_socket(&socket_path) {
        let _ = fs::remove_file(&socket_path);
        return Err(error);
    }
    let meta = fs::symlink_metadata(&socket_path).map_err(|error| {
        let _ = fs::remove_file(&socket_path);
        InstanceError::new(error.to_string())
    })?;
    listener.set_nonblocking(true).map_err(|error| {
        let _ = fs::remove_file(&socket_path);
        InstanceError::new(error.to_string())
    })?;
    let (wake_read, wake_write) = UnixStream::pair().map_err(|error| {
        let _ = fs::remove_file(&socket_path);
        InstanceError::new(error.to_string())
    })?;
    wake_read.set_nonblocking(true).map_err(|error| {
        let _ = fs::remove_file(&socket_path);
        InstanceError::new(error.to_string())
    })?;
    let shared = SharedState::new();
    let thread_shared = Arc::clone(&shared);
    let thread = thread::Builder::new()
        .name("clipsync-activate".into())
        .spawn(move || accept_loop(listener, wake_read, thread_shared))
        .map_err(|error| {
            let _ = fs::remove_file(&socket_path);
            InstanceError::new(error.to_string())
        })?;
    Ok(Launch::Primary(Guard {
        _lock: lock,
        socket_path,
        socket_dev: meta.dev(),
        socket_ino: meta.ino(),
        wake: Some(wake_write),
        thread: Some(thread),
        shared,
    }))
}

fn accept_loop(listener: UnixListener, wake: UnixStream, shared: Arc<SharedState>) {
    loop {
        let mut fds = [
            libc::pollfd {
                fd: listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if rc < 0 {
            if std::io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if fds[1].revents != 0 {
            break;
        }
        if fds[0].revents == 0 {
            continue;
        }
        match listener.accept() {
            Ok((stream, _)) => handle_client(stream, &shared),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted
                ) => {}
            // Resource exhaustion can be temporary; keep ownership and allow
            // the wake socket to stop the listener on the next poll.
            Err(_) => thread::sleep(SIGNAL_PAUSE),
        }
    }
}

fn handle_client(mut stream: UnixStream, shared: &SharedState) {
    if peer_uid(&stream).ok() != Some(current_uid()) {
        return;
    }
    if stream.set_write_timeout(Some(IO_TIMEOUT)).is_err() {
        return;
    }
    if read_bounded(&mut stream) {
        shared.signal();
        let _ = stream.write_all(ACTIVATION_OK);
    }
}

fn read_bounded(stream: &mut UnixStream) -> bool {
    let mut buf = [0u8; MAX_ACTIVATION];
    let mut filled = 0usize;
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return false;
        };
        if stream.set_read_timeout(Some(remaining)).is_err() {
            return false;
        }
        if filled == MAX_ACTIVATION {
            return false;
        }
        match stream.read(&mut buf[filled..]) {
            Ok(0) => return &buf[..filled] == ACTIVATION,
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
}

fn signal_owner(socket_path: &Path) -> Result<Launch, InstanceError> {
    let mut last = InstanceError::new("existing instance did not accept activation");
    for attempt in 0..SIGNAL_ATTEMPTS {
        if attempt > 0 {
            thread::sleep(SIGNAL_PAUSE);
        }
        let mut stream = match UnixStream::connect(socket_path) {
            Ok(stream) => stream,
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    || error.kind() == io::ErrorKind::ConnectionRefused =>
            {
                last = InstanceError::new(format!("existing instance socket not ready: {error}"));
                continue;
            }
            Err(error) => {
                return Err(InstanceError::new(format!(
                    "activation connect failed: {error}"
                )));
            }
        };
        if peer_uid(&stream).ok() != Some(current_uid()) {
            return Err(InstanceError::new(
                "activation peer belongs to another user",
            ));
        }
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        if stream.write_all(ACTIVATION).is_err() {
            last = InstanceError::new("activation write failed");
            continue;
        }
        if stream.shutdown(std::net::Shutdown::Write).is_err() {
            last = InstanceError::new("activation shutdown failed");
            continue;
        }
        let mut reply = [0u8; 16];
        match stream.read(&mut reply) {
            Ok(n) if &reply[..n] == ACTIVATION_OK => return Ok(Launch::Signaled),
            Ok(_) => return Err(InstanceError::new("activation rejected")),
            Err(error) => last = InstanceError::new(format!("activation reply failed: {error}")),
        }
    }
    Err(last)
}

fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    #[cfg(target_os = "linux")]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut libc::ucred as *mut libc::c_void,
                &mut len,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(cred.uid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(uid)
    }
}

fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

fn desktop_runtime_dir() -> Result<PathBuf, InstanceError> {
    // GUI launchers and terminals may supply different XDG_RUNTIME_DIR/TMPDIR.
    // One config must still find the same owner. Use a stable system location;
    // all entries beneath it are checked for ownership and private permissions.
    #[cfg(target_os = "linux")]
    let base = {
        let runtime = PathBuf::from(format!("/run/user/{}", current_uid()));
        if runtime.is_dir() {
            runtime
        } else {
            PathBuf::from("/tmp")
        }
    };
    #[cfg(not(target_os = "linux"))]
    let base = PathBuf::from("/tmp");
    Ok(base.join(format!("clipsync-desktop-{}", current_uid())))
}

fn private_directory(path: &Path) -> Result<(), InstanceError> {
    let created = match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) => {
            return Err(InstanceError::new(format!(
                "instance directory unavailable: {error}"
            )));
        }
    };
    if created {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| InstanceError::new(error.to_string()))?;
    }
    let meta = fs::symlink_metadata(path)
        .map_err(|error| InstanceError::new(format!("instance directory unavailable: {error}")))?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != current_uid()
        || meta.mode() & 0o077 != 0
    {
        return Err(InstanceError::new(
            "instance directory must be owned by this user with mode 0700",
        ));
    }
    Ok(())
}

fn open_lock(path: &Path) -> Result<File, InstanceError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| InstanceError::new(format!("instance lock open failed: {error}")))?;
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(file.as_raw_fd(), &mut stat) } != 0 {
        return Err(InstanceError::new(
            "instance lock could not be inspected".to_string(),
        ));
    }
    let mode = stat.st_mode;
    if stat.st_uid != current_uid() || mode & libc::S_IFMT != libc::S_IFREG {
        return Err(InstanceError::new(
            "instance lock must be a regular file owned by this user",
        ));
    }
    if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
        return Err(InstanceError::new(
            "instance lock must be mode 0600".to_string(),
        ));
    }
    if unsafe { libc::fstat(file.as_raw_fd(), &mut stat) } != 0 || stat.st_mode as u32 & 0o077 != 0
    {
        return Err(InstanceError::new(
            "instance lock must be mode 0600".to_string(),
        ));
    }
    Ok(file)
}

fn replace_stale_socket(path: &Path) -> Result<(), InstanceError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(InstanceError::new(format!(
                "instance socket unavailable: {error}"
            )));
        }
    };
    match socket_disposition(
        meta.file_type().is_symlink(),
        meta.file_type().is_socket(),
        meta.uid(),
        current_uid(),
    ) {
        SocketDisposition::Replace => fs::remove_file(path)
            .map_err(|error| InstanceError::new(format!("stale instance socket remains: {error}"))),
        SocketDisposition::RefuseOtherOwner => Err(InstanceError::new(
            "instance socket belongs to another user",
        )),
        SocketDisposition::RefuseUnsafe => Err(InstanceError::new(
            "instance socket must be a regular socket owned by this user",
        )),
    }
}

fn seal_socket(path: &Path) -> Result<(), InstanceError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| InstanceError::new(error.to_string()))?;
    let meta = fs::symlink_metadata(path).map_err(|error| InstanceError::new(error.to_string()))?;
    if meta.file_type().is_symlink()
        || !meta.file_type().is_socket()
        || meta.uid() != current_uid()
        || meta.mode() & 0o077 != 0
    {
        return Err(InstanceError::new(
            "instance socket must be owned by this user with mode 0600",
        ));
    }
    Ok(())
}

pub(crate) fn artifact_names(material: &[u8]) -> (String, String) {
    let digest = Sha256::digest(material);
    let mut hex = String::with_capacity(20);
    for byte in digest.iter().take(10) {
        hex.push_str(&format!("{byte:02x}"));
    }
    (format!("{hex}.sock"), format!("{hex}.lock"))
}

/// Stable instance key. Explicit `--config` wins. Otherwise resolution covers
/// `CLIPSYNC_CONFIG`, the canonical XDG path, and conflicts.
pub(crate) fn identity_material(
    explicit: Option<&Path>,
    paths: &ResolvePaths,
) -> Result<Vec<u8>, InstanceError> {
    if let Some(path) = explicit {
        return Ok(encode_config(&stable_path(path)?));
    }
    let resolution = resolve_config(paths, None);
    match resolution.outcome {
        Ok(ResolveOutcome::Selected { path, .. })
        | Ok(ResolveOutcome::FreshEligible {
            proposed_path: path,
        }) => Ok(encode_config(&stable_path(&path)?)),
        Ok(ResolveOutcome::Conflict { candidates }) => {
            let mut keys = Vec::with_capacity(candidates.len());
            for candidate in candidates {
                keys.push(stable_path(&candidate.path)?);
            }
            keys.sort();
            keys.dedup();
            let mut material = b"conflict\0".to_vec();
            for key in keys {
                material.extend(path_bytes(&key));
                material.push(0);
            }
            Ok(material)
        }
        Err(issue) => {
            let path = issue
                .path
                .unwrap_or_else(|| paths.canonical_xdg_config_toml());
            Ok(encode_config(&stable_path(&path)?))
        }
    }
}

fn encode_config(path: &Path) -> Vec<u8> {
    let mut material = b"config\0".to_vec();
    material.extend(path_bytes(path));
    material
}

fn path_bytes(path: &Path) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes()
}

/// Absolute path with `.` and `..` removed, then the longest existing prefix
/// canonicalized so a missing first-run file matches the later created file.
pub(crate) fn stable_path(path: &Path) -> Result<PathBuf, InstanceError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| InstanceError::new(error.to_string()))?
            .join(path)
    };
    // Resolve symlinks before interpreting .., as the filesystem does.
    let mut cleaned = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                cleaned.pop();
            }
            other => {
                cleaned.push(other.as_os_str());
                if let Ok(canonical) = fs::canonicalize(&cleaned) {
                    cleaned = canonical;
                }
            }
        }
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clipsync::config::resolution::ResolvePaths;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    static SCRATCH: AtomicU64 = AtomicU64::new(1);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let n = SCRATCH.fetch_add(1, AtomicOrdering::Relaxed);
            let path = std::env::temp_dir().join(format!("cdi-{}-{}", std::process::id(), n));
            fs::create_dir_all(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn minimal_config(path: &Path, port: u16) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                r#"listen_addr = "127.0.0.1:{port}"
advertise_name = "fixture"
log_level = "info"
[auth]
ssh_key = "/tmp/clipsync-instance-ssh"
authorized_keys = "/tmp/clipsync-instance-auth"
[clipboard]
max_size = 1048576
sync_primary = true
history_size = 20
history_db = "/tmp/clipsync-instance-history.db"
"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn explicit_aliases_share_identity_and_distinct_paths_do_not() {
        let scratch = Scratch::new();
        let file = scratch.path().join("nested/config.toml");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"explicit").unwrap();
        let via = scratch.path().join("nested/sub/../config.toml");
        let alias = scratch.path().join("alias.toml");
        symlink(&file, &alias).unwrap();
        let other = scratch.path().join("other.toml");
        fs::write(&other, b"other").unwrap();
        let paths = ResolvePaths::for_fixture(scratch.path());
        let direct = identity_material(Some(&file), &paths).unwrap();
        assert_eq!(direct, identity_material(Some(&via), &paths).unwrap());
        assert_eq!(direct, identity_material(Some(&alias), &paths).unwrap());
        assert_ne!(direct, identity_material(Some(&other), &paths).unwrap());
    }

    #[test]
    fn parent_components_follow_symlink_targets() {
        let scratch = Scratch::new();
        let target = scratch.path().join("actual/deep");
        fs::create_dir_all(&target).unwrap();
        let alias = scratch.path().join("alias");
        symlink(&target, &alias).unwrap();
        let actual = scratch.path().join("actual/config.toml");
        let through_alias = alias.join("../config.toml");
        assert_eq!(
            stable_path(&actual).unwrap(),
            stable_path(&through_alias).unwrap()
        );
        fs::write(&actual, b"fixture").unwrap();
        assert_eq!(
            stable_path(&actual).unwrap(),
            stable_path(&through_alias).unwrap()
        );
        assert_ne!(
            stable_path(&scratch.path().join("config.toml")).unwrap(),
            stable_path(&through_alias).unwrap()
        );
    }

    #[test]
    fn slowly_fragmented_activation_has_a_total_deadline() {
        let (mut receiver, mut sender) = UnixStream::pair().unwrap();
        let writer = thread::spawn(move || {
            let _ = sender.write_all(b"v");
            thread::sleep(Duration::from_millis(650));
            let _ = sender.write_all(b"1");
            thread::sleep(Duration::from_millis(650));
            let _ = sender.write_all(b"\n");
        });
        assert!(!read_bounded(&mut receiver));
        writer.join().unwrap();
    }

    #[test]
    fn default_identity_matches_before_and_after_first_run_file() {
        let scratch = Scratch::new();
        let paths = ResolvePaths::for_fixture(scratch.path());
        let before = identity_material(None, &paths).unwrap();
        minimal_config(&paths.canonical_xdg_config_toml(), 8484);
        let after = identity_material(None, &paths).unwrap();
        assert_eq!(before, after);
        assert_eq!(
            before,
            identity_material(Some(&paths.canonical_xdg_config_toml()), &paths).unwrap()
        );
    }

    #[test]
    fn missing_explicit_and_env_use_the_same_stable_path() {
        let scratch = Scratch::new();
        let missing = scratch.path().join("missing.toml");
        let paths =
            ResolvePaths::for_fixture(scratch.path()).with_clipsync_config(Some(missing.clone()));
        assert_eq!(
            identity_material(None, &paths).unwrap(),
            identity_material(Some(&missing), &paths).unwrap()
        );
        let elsewhere = scratch.path().join("elsewhere.toml");
        assert_ne!(
            identity_material(Some(&missing), &paths).unwrap(),
            identity_material(Some(&elsewhere), &paths).unwrap()
        );
    }

    #[test]
    fn distinct_existing_configs_use_a_shared_conflict_identity() {
        let scratch = Scratch::new();
        let paths = ResolvePaths::for_fixture(scratch.path());
        minimal_config(&paths.canonical_xdg_config_toml(), 8484);
        minimal_config(&paths.legacy_mac_config_toml(), 9999);
        let first = identity_material(None, &paths).unwrap();
        let second = identity_material(None, &paths).unwrap();
        assert_eq!(first, second);
        assert_ne!(
            first,
            identity_material(Some(&paths.canonical_xdg_config_toml()), &paths).unwrap()
        );
    }

    #[test]
    fn duplicate_config_signals_and_distinct_config_coexists() {
        let scratch = Scratch::new();
        let runtime = scratch.path().join("runtime");
        let primary = match claim_at(b"config\0alpha", &runtime).unwrap() {
            Launch::Primary(guard) => guard,
            Launch::Signaled => panic!("first claim must own the instance"),
        };
        match claim_at(b"config\0alpha", &runtime).unwrap() {
            Launch::Signaled => {}
            Launch::Primary(_) => panic!("duplicate config started a second owner"),
        }
        assert!(primary.take_activation());
        let other = match claim_at(b"config\0beta", &runtime).unwrap() {
            Launch::Primary(guard) => guard,
            Launch::Signaled => panic!("distinct config must coexist"),
        };
        assert!(!other.take_activation());
        drop(other);
        drop(primary);
        let (socket_name, lock_name) = artifact_names(b"config\0alpha");
        assert!(!runtime.join(socket_name).exists());
        assert!(runtime.join(lock_name).exists());
    }

    #[test]
    fn stale_owned_socket_is_replaced_and_symlink_is_not_removed() {
        let scratch = Scratch::new();
        let runtime = scratch.path().join("runtime");
        fs::create_dir_all(&runtime).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        let (socket_name, _) = artifact_names(b"config\0stale");
        let socket_path = runtime.join(&socket_name);
        drop(UnixListener::bind(&socket_path).unwrap());
        let guard = match claim_at(b"config\0stale", &runtime).unwrap() {
            Launch::Primary(guard) => guard,
            Launch::Signaled => panic!("stale socket must not count as a live owner"),
        };
        // Filesystems may immediately reuse the removed inode. Prove that the
        // replacement listener accepts activation, not that its inode changed.
        assert!(matches!(
            claim_at(b"config\0stale", &runtime).unwrap(),
            Launch::Signaled
        ));
        assert!(guard.take_activation());
        drop(guard);

        let target = runtime.join("elsewhere.sock");
        drop(UnixListener::bind(&target).unwrap());
        symlink(&target, &socket_path).unwrap();
        assert!(claim_at(b"config\0stale", &runtime).is_err());
        assert!(
            fs::symlink_metadata(&socket_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(target.exists());
    }

    #[test]
    fn unsafe_directory_and_lock_symlink_are_left_alone() {
        let scratch = Scratch::new();
        let loose = scratch.path().join("loose");
        fs::create_dir_all(&loose).unwrap();
        fs::set_permissions(&loose, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(claim_at(b"config\0loose", &loose).is_err());
        assert_eq!(fs::symlink_metadata(&loose).unwrap().mode() & 0o777, 0o777);

        let runtime = scratch.path().join("runtime");
        fs::create_dir_all(&runtime).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        let (_, lock_name) = artifact_names(b"config\0link");
        let lock_path = runtime.join(lock_name);
        let real = runtime.join("real.lock");
        fs::write(&real, b"lock").unwrap();
        symlink(&real, &lock_path).unwrap();
        assert!(claim_at(b"config\0link", &runtime).is_err());
        assert!(
            fs::symlink_metadata(&lock_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn other_owner_socket_is_not_a_removal_candidate() {
        assert_eq!(
            socket_disposition(false, true, 1, 2),
            SocketDisposition::RefuseOtherOwner
        );
        assert_eq!(
            socket_disposition(true, true, 2, 2),
            SocketDisposition::RefuseUnsafe
        );
        assert_eq!(
            socket_disposition(false, false, 2, 2),
            SocketDisposition::RefuseUnsafe
        );
        assert_eq!(
            socket_disposition(false, true, 2, 2),
            SocketDisposition::Replace
        );
    }

    #[test]
    fn oversized_handshake_does_not_show_the_window() {
        let scratch = Scratch::new();
        let runtime = scratch.path().join("runtime");
        let guard = match claim_at(b"config\0noise", &runtime).unwrap() {
            Launch::Primary(guard) => guard,
            Launch::Signaled => panic!("owner expected"),
        };
        let (socket_name, _) = artifact_names(b"config\0noise");
        let mut stream = UnixStream::connect(runtime.join(socket_name)).unwrap();
        stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
        stream.write_all(&[b'x'; MAX_ACTIVATION]).unwrap();
        let _ = stream.shutdown(std::net::Shutdown::Write);
        thread::sleep(Duration::from_millis(50));
        assert!(!guard.take_activation());
        drop(stream);
        match claim_at(b"config\0noise", &runtime).unwrap() {
            Launch::Signaled => {}
            Launch::Primary(_) => panic!("owner was lost after a bad handshake"),
        }
        assert!(guard.take_activation());
    }
}
