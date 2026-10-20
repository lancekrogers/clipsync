//! Managed per-user service paths and validation.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::error::ServiceError;
use super::host::is_safe_absolute;

/// systemd user unit basename.
pub const UNIT_NAME: &str = "clipsync.service";

/// launchd LaunchAgent label.
pub const LAUNCH_AGENT_LABEL: &str = "com.clipsync";

/// LaunchAgent plist filename.
pub const LAUNCH_AGENT_PLIST_NAME: &str = "com.clipsync.plist";

/// Fixture-friendly path roots for service operations.
#[derive(Debug, Clone)]
pub struct ServicePaths {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub uid: u32,
    /// Overlay for legacy system unit inspection (defaults to production paths).
    pub legacy_system_units: Vec<PathBuf>,
}

impl ServicePaths {
    /// Build paths with production legacy unit locations.
    pub fn new(home: impl Into<PathBuf>, xdg_config_home: impl Into<PathBuf>, uid: u32) -> Self {
        Self {
            home: home.into(),
            xdg_config_home: xdg_config_home.into(),
            uid,
            legacy_system_units: crate::service_install::legacy_system_unit_paths()
                .iter()
                .map(PathBuf::from)
                .collect(),
        }
    }

    /// True when any configured legacy system unit file exists (inspect-only).
    pub fn legacy_root_present(&self) -> bool {
        self.legacy_system_units.iter().any(|path| path.is_file())
    }

    pub fn launch_agent_plist(&self) -> PathBuf {
        self.home
            .join("Library")
            .join("LaunchAgents")
            .join(LAUNCH_AGENT_PLIST_NAME)
    }

    pub fn systemd_user_unit(&self) -> PathBuf {
        self.xdg_config_home
            .join("systemd")
            .join("user")
            .join(UNIT_NAME)
    }

    pub fn launchd_gui_domain(&self) -> String {
        format!("gui/{}", self.uid)
    }

    pub fn launchd_service_target(&self) -> String {
        format!("gui/{}/{}", self.uid, LAUNCH_AGENT_LABEL)
    }
}

/// Explicit absolute binary + config for unit/plist rendering.
#[derive(Debug, Clone)]
pub struct ServiceTarget {
    pub binary: PathBuf,
    pub config: PathBuf,
}

impl ServiceTarget {
    pub fn new(
        binary: impl Into<PathBuf>,
        config: impl Into<PathBuf>,
    ) -> Result<Self, ServiceError> {
        let binary = binary.into();
        let config = config.into();
        validate_managed_file_path(&binary, "daemon binary")?;
        validate_managed_file_path(&config, "config")?;
        Ok(Self { binary, config })
    }

    pub fn require_executable(&self) -> Result<(), ServiceError> {
        let meta = fs::metadata(&self.binary).map_err(|err| ServiceError::PathInvalid {
            path: self.binary.clone(),
            reason: format!("binary not readable: {err}"),
        })?;
        if !meta.is_file() {
            return Err(ServiceError::PathInvalid {
                path: self.binary.clone(),
                reason: "binary path is not a regular file".into(),
            });
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o111 == 0 {
                return Err(ServiceError::PathInvalid {
                    path: self.binary.clone(),
                    reason: "binary is not executable".into(),
                });
            }
        }
        Ok(())
    }
}

/// Validate an absolute path suitable for embedding in a managed unit (no `..`, no control chars).
pub fn validate_managed_file_path(path: &Path, role: &str) -> Result<(), ServiceError> {
    if !is_safe_absolute(path) {
        return Err(ServiceError::PathInvalid {
            path: path.to_path_buf(),
            reason: format!("{role} must be an absolute path without '..'"),
        });
    }
    let text = path.to_string_lossy();
    if text.chars().any(char::is_control) {
        return Err(ServiceError::PathInvalid {
            path: path.to_path_buf(),
            reason: format!("{role} contains unsupported control characters"),
        });
    }
    Ok(())
}

/// Refuse symlinks at managed destinations (do not follow into unexpected ownership).
pub fn refuse_symlink(path: &Path) -> Result<(), ServiceError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(ServiceError::PathInvalid {
            path: path.to_path_buf(),
            reason: "managed path must not be a symlink".into(),
        }),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(ServiceError::io(format!("stat {}", path.display()), err)),
    }
}

/// Ensure the managed parent directory exists.
///
/// Existing directories keep their permissions (never chmod pre-existing parents).
/// Only newly created components receive `mode`. Symlinked destinations are refused.
pub fn ensure_managed_dir(dir: &Path, mode: u32) -> Result<(), ServiceError> {
    validate_managed_file_path(dir, "managed directory")?;
    refuse_symlink(dir)?;

    if dir.exists() {
        let meta = fs::symlink_metadata(dir)
            .map_err(|err| ServiceError::io(format!("stat {}", dir.display()), err))?;
        if meta.file_type().is_symlink() {
            return Err(ServiceError::PathInvalid {
                path: dir.to_path_buf(),
                reason: "managed directory must not be a symlink".into(),
            });
        }
        if !meta.is_dir() {
            return Err(ServiceError::PathInvalid {
                path: dir.to_path_buf(),
                reason: "managed path exists and is not a directory".into(),
            });
        }
        return Ok(());
    }

    create_dir_components(dir, mode)
}

fn create_dir_components(dir: &Path, mode: u32) -> Result<(), ServiceError> {
    let mut built = PathBuf::new();
    for component in dir.components() {
        built.push(component);
        if built.as_os_str().is_empty() || built == Path::new("/") {
            continue;
        }
        match fs::symlink_metadata(&built) {
            Ok(meta) => {
                // Existing parents may be system symlinks (e.g. macOS /var → /private/var).
                // Only the final managed destination is refused as a symlink (caller checks).
                if meta.file_type().is_symlink() {
                    // Resolve one hop to ensure it points at a directory.
                    let meta = fs::metadata(&built).map_err(|e| {
                        ServiceError::io(format!("stat through {}", built.display()), e)
                    })?;
                    if !meta.is_dir() {
                        return Err(ServiceError::PathInvalid {
                            path: built,
                            reason: "symlink path component does not resolve to a directory".into(),
                        });
                    }
                    continue;
                }
                if !meta.is_dir() {
                    return Err(ServiceError::PathInvalid {
                        path: built,
                        reason: "path component exists and is not a directory".into(),
                    });
                }
                // Existing real directory: preserve permissions (never chmod).
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // Only newly created components get `mode`; never chmod pre-existing parents.
                fs::create_dir(&built)
                    .map_err(|e| ServiceError::io(format!("create {}", built.display()), e))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = fs::metadata(&built)
                        .map_err(|e| ServiceError::io(format!("stat {}", built.display()), e))?
                        .permissions();
                    perms.set_mode(mode);
                    fs::set_permissions(&built, perms)
                        .map_err(|e| ServiceError::io(format!("chmod {}", built.display()), e))?;
                }
                let _ = mode;
            }
            Err(err) => {
                return Err(ServiceError::io(format!("stat {}", built.display()), err));
            }
        }
    }
    Ok(())
}

/// Atomically replace `dest` with `content`, returning previous bytes if any.
///
/// Temp files use `create_new` with private mode first; destination symlinks are refused.
/// File and parent directory are fsynced when the platform supports it.
pub fn atomic_write(
    dest: &Path,
    content: &str,
    mode: u32,
) -> Result<Option<Vec<u8>>, ServiceError> {
    validate_managed_file_path(dest, "managed unit file")?;
    refuse_symlink(dest)?;
    let parent = dest.parent().ok_or_else(|| ServiceError::PathInvalid {
        path: dest.to_path_buf(),
        reason: "managed file has no parent directory".into(),
    })?;
    ensure_managed_dir(parent, 0o755)?;

    let previous = match fs::read(dest) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(ServiceError::io(format!("read {}", dest.display()), err)),
    };

    let temp = dest.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    refuse_symlink(&temp)?;

    write_temp_private(&temp, content.as_bytes(), mode)?;
    fs::rename(&temp, dest).map_err(|err| {
        let _ = fs::remove_file(&temp);
        ServiceError::io(format!("rename to {}", dest.display()), err)
    })?;
    fsync_path(dest)?;
    fsync_path(parent)?;
    Ok(previous)
}

fn write_temp_private(path: &Path, content: &[u8], mode: u32) -> Result<(), ServiceError> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Private until we chmod to the desired mode after write.
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .map_err(|err| ServiceError::io(format!("create_new {}", path.display()), err))?;
    file.write_all(content)
        .map_err(|err| ServiceError::io(format!("write {}", path.display()), err))?;
    file.sync_all()
        .map_err(|err| ServiceError::io(format!("fsync {}", path.display()), err))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = file
            .metadata()
            .map_err(|err| ServiceError::io(format!("stat {}", path.display()), err))?
            .permissions();
        perms.set_mode(mode);
        fs::set_permissions(path, perms)
            .map_err(|err| ServiceError::io(format!("chmod {}", path.display()), err))?;
    }
    let _ = mode;
    Ok(())
}

fn fsync_path(path: &Path) -> Result<(), ServiceError> {
    let file = File::open(path)
        .map_err(|err| ServiceError::io(format!("open for fsync {}", path.display()), err))?;
    file.sync_all()
        .map_err(|err| ServiceError::io(format!("fsync {}", path.display()), err))?;
    Ok(())
}

/// Restore previous content or remove the file if there was none.
pub fn restore_previous(dest: &Path, previous: Option<&[u8]>) -> Result<(), ServiceError> {
    refuse_symlink(dest)?;
    match previous {
        Some(bytes) => {
            let parent = dest.parent().ok_or_else(|| ServiceError::PathInvalid {
                path: dest.to_path_buf(),
                reason: "managed file has no parent directory".into(),
            })?;
            ensure_managed_dir(parent, 0o755)?;
            let mode = match fs::metadata(dest) {
                Ok(meta) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        meta.permissions().mode()
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = meta;
                        0o644
                    }
                }
                Err(_) => 0o644,
            };
            let temp = dest.with_extension(format!(
                "restore-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            refuse_symlink(&temp)?;
            write_temp_private(&temp, bytes, mode)?;
            fs::rename(&temp, dest).map_err(|err| {
                let _ = fs::remove_file(&temp);
                ServiceError::io(format!("restore rename to {}", dest.display()), err)
            })?;
            fsync_path(dest)?;
            fsync_path(parent)?;
        }
        None => {
            if dest.exists() {
                refuse_symlink(dest)?;
                fs::remove_file(dest)
                    .map_err(|err| ServiceError::io(format!("remove {}", dest.display()), err))?;
            }
        }
    }
    Ok(())
}

/// Confirm `candidate` is exactly the managed path (string-equal after normalization).
pub fn require_exact_managed(candidate: &Path, expected: &Path) -> Result<(), ServiceError> {
    if candidate != expected {
        return Err(ServiceError::NotManagedUnit {
            expected: expected.display().to_string(),
            actual: candidate.display().to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[test]
    fn service_paths_are_exact() {
        let paths = ServicePaths::new("/tmp/home", "/tmp/xdg", 501);
        assert_eq!(
            paths.launch_agent_plist(),
            PathBuf::from("/tmp/home/Library/LaunchAgents/com.clipsync.plist")
        );
        assert_eq!(
            paths.systemd_user_unit(),
            PathBuf::from("/tmp/xdg/systemd/user/clipsync.service")
        );
        assert_eq!(paths.launchd_service_target(), "gui/501/com.clipsync");
    }

    #[test]
    fn atomic_write_roundtrip_and_restore() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("clipsync.service");
        let prev = atomic_write(&dest, "one\n", 0o644).unwrap();
        assert!(prev.is_none());
        let prev = atomic_write(&dest, "two\n", 0o644).unwrap();
        assert_eq!(prev.as_deref(), Some(b"one\n".as_slice()));
        restore_previous(&dest, prev.as_deref()).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "one\n");
    }

    #[test]
    fn ensure_managed_dir_preserves_existing_mode() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("already");
        fs::create_dir(&nested).unwrap();
        let mut perms = fs::metadata(&nested).unwrap().permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&nested, perms).unwrap();
        ensure_managed_dir(&nested, 0o755).unwrap();
        let mode = fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn refuse_symlink_destination() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real");
        let link = dir.path().join("link");
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = ensure_managed_dir(&link, 0o755).unwrap_err();
        assert!(matches!(err, ServiceError::PathInvalid { .. }));
    }
}
