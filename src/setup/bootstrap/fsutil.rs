//! Private directories, O_NOFOLLOW create_new writes, symlink guards.

use std::fs;
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;

use super::error::BootstrapError;

pub(super) fn ensure_private_dir(
    path: &Path,
    mode: u32,
    home_root: &Path,
) -> Result<(), BootstrapError> {
    if path.exists() {
        reject_symlink(path)?;
        let meta = fs::symlink_metadata(path)?;
        if !meta.file_type().is_dir() {
            return Err(BootstrapError::Other(format!(
                "expected directory: {}",
                path.display()
            )));
        }
    } else {
        // Create parents; then verify no component under home_root is a symlink.
        // Do not inspect OS paths above the fixture/home (e.g. macOS /var → /private/var).
        if let Some(parent) = path.parent() {
            if parent.starts_with(home_root) || parent == home_root {
                reject_symlink_under_root(parent, home_root)?;
            }
        }
        fs::create_dir_all(path)?;
        reject_symlink_under_root(path, home_root)?;
    }
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(mode);
        fs::set_permissions(path, perms)?;
    }
    let _ = mode;
    Ok(())
}

pub(super) fn reject_symlink(path: &Path) -> Result<(), BootstrapError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(BootstrapError::SymlinkRejected(path.to_path_buf()))
        }
        Ok(_) | Err(_) => Ok(()),
    }
}

/// Reject symlinks on `path` and ancestors down to `root`.
pub(super) fn reject_symlink_under_root(path: &Path, root: &Path) -> Result<(), BootstrapError> {
    let mut cur = path.to_path_buf();
    loop {
        if let Ok(meta) = fs::symlink_metadata(&cur) {
            if meta.file_type().is_symlink() {
                return Err(BootstrapError::SymlinkRejected(cur));
            }
        }
        if cur == root {
            break;
        }
        match cur.parent() {
            Some(parent) if parent != cur => {
                if !cur.starts_with(root) && cur != root {
                    break;
                }
                cur = parent.to_path_buf();
            }
            _ => break,
        }
    }
    Ok(())
}

pub(super) fn write_create_new(path: &Path, data: &[u8], mode: u32) -> Result<(), BootstrapError> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(BootstrapError::SymlinkRejected(path.to_path_buf()));
        }
        return Err(BootstrapError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("refusing to clobber {}", path.display()),
        )));
    }
    if let Some(parent) = path.parent() {
        reject_symlink(parent)?;
    }
    #[cfg(unix)]
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true).mode(mode);
        opts.custom_flags(libc::O_NOFOLLOW);
        let mut file = opts.open(path)?;
        file.write_all(data)?;
        file.sync_all()?;
        return Ok(());
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(data)?;
        file.sync_all()?;
        Ok(())
    }
}

/// Publish temp → final via hard_link (no-clobber). Removes only `tmp`.
pub(super) fn publish_hard_link_no_clobber(
    tmp: &Path,
    final_path: &Path,
) -> Result<(), BootstrapError> {
    match fs::hard_link(tmp, final_path) {
        Ok(()) => {
            let _ = fs::remove_file(tmp);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(tmp);
            Err(BootstrapError::Io(e))
        }
        Err(e) => {
            let _ = fs::remove_file(tmp);
            Err(BootstrapError::Io(e))
        }
    }
}

pub(super) fn sync_dir(path: &Path) -> Result<(), BootstrapError> {
    let f = fs::File::open(path)?;
    f.sync_all()?;
    Ok(())
}

/// Exclusive non-blocking flock on setup.lock (paths/fingerprints only — no secrets).
pub(super) struct SetupLock {
    _file: fs::File,
}

impl SetupLock {
    pub(super) fn try_acquire(
        lock_path: &Path,
        state_dir: &Path,
        home_root: &Path,
    ) -> Result<Self, BootstrapError> {
        ensure_private_dir(state_dir, 0o700, home_root)?;
        #[cfg(unix)]
        {
            let mut opts = fs::OpenOptions::new();
            opts.create(true).read(true).write(true).mode(0o600);
            opts.custom_flags(libc::O_NOFOLLOW);
            let file = opts.open(lock_path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::Other
                    || e.raw_os_error() == Some(libc::ELOOP as i32)
                {
                    BootstrapError::SymlinkRejected(lock_path.to_path_buf())
                } else {
                    BootstrapError::Io(e)
                }
            })?;
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc != 0 {
                return Err(BootstrapError::SetupInProgress);
            }
            return Ok(Self { _file: file });
        }
        #[cfg(not(unix))]
        {
            let _ = (lock_path, state_dir, home_root);
            Err(BootstrapError::Other(
                "setup lock requires unix flock".into(),
            ))
        }
    }
}
