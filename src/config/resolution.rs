//! Canonical configuration discovery (D02).
//!
//! Pure resolve/inspect: never mutates disk. Hard-fails missing explicit /
//! `CLIPSYNC_CONFIG` paths. Surfaces XDG vs legacy macOS Application Support
//! conflicts instead of silently picking one.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use thiserror::Error;

use super::{Config, ConfigError};

/// How a config path was chosen or proposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigProvenance {
    ExplicitFlag,
    ExplicitApi,
    EnvClipSyncConfig,
    CanonicalXdg,
    LegacyMacAppSupport,
    DirsConfigDir,
    ServiceUnit,
}

/// Explicit override kind (CLI `--config` vs companion API).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplicitKind {
    Flag,
    Api,
}

impl ExplicitKind {
    fn provenance(self) -> ConfigProvenance {
        match self {
            ExplicitKind::Flag => ConfigProvenance::ExplicitFlag,
            ExplicitKind::Api => ConfigProvenance::ExplicitApi,
        }
    }
}

/// Content fingerprint for conflict detection (size + mtime + sha256).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentFingerprint {
    pub size: u64,
    pub mtime_secs: Option<u64>,
    pub sha256_hex: String,
}

impl ContentFingerprint {
    pub fn of_file(path: &Path) -> std::io::Result<Self> {
        let meta = fs::symlink_metadata(path)?;
        if !meta.file_type().is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path is not a regular file",
            ));
        }
        let data = fs::read(path)?;
        let mtime_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        let mut hasher = Sha256::new();
        hasher.update(&data);
        Ok(Self {
            size: data.len() as u64,
            mtime_secs,
            sha256_hex: hex::encode(hasher.finalize()),
        })
    }

    pub fn same_content(&self, other: &Self) -> bool {
        self.sha256_hex == other.sha256_hex && self.size == other.size
    }
}

/// A discovered config file candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigCandidate {
    pub path: PathBuf,
    pub provenance: ConfigProvenance,
    pub fingerprint: ContentFingerprint,
}

/// Stable error vocabulary for resolution (D02).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolveErrorCode {
    MissingExplicitConfig,
    ConfigConflict,
    /// Canonical candidate exists but is a symlink, non-regular, or unreadable.
    ConfigUnusable,
    ConfigParse,
    ConfigValidation,
    IdentityMissing,
    IdentityInsecurePerms,
    IdentityUnreadable,
    AuthFileMissing,
    AuthFileIsGeneralSsh,
    HistoryKeyMissingForExistingDb,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveIssue {
    pub code: ResolveErrorCode,
    pub message: String,
    pub path: Option<PathBuf>,
}

/// Outcome of pure config resolution.
#[derive(Debug, Clone)]
pub enum ResolveOutcome {
    /// A single config path was selected and parsed.
    Selected {
        path: PathBuf,
        provenance: ConfigProvenance,
        config: Config,
    },
    /// No config files exist; companion must create canonical fresh layout.
    FreshEligible { proposed_path: PathBuf },
    /// Multiple distinct configs; caller must choose before write/start.
    Conflict { candidates: Vec<ConfigCandidate> },
}

/// Full structured resolution result (never mutates disk).
#[derive(Debug, Clone)]
pub struct ConfigResolution {
    pub outcome: Result<ResolveOutcome, ResolveIssue>,
    pub candidates: Vec<ConfigCandidate>,
    pub warnings: Vec<ResolveIssue>,
    pub errors: Vec<ResolveIssue>,
}

impl ConfigResolution {
    pub fn selected_path(&self) -> Option<&Path> {
        match &self.outcome {
            Ok(ResolveOutcome::Selected { path, .. }) => Some(path.as_path()),
            Ok(ResolveOutcome::FreshEligible { proposed_path }) => Some(proposed_path.as_path()),
            _ => None,
        }
    }

    pub fn is_fresh_eligible(&self) -> bool {
        matches!(self.outcome, Ok(ResolveOutcome::FreshEligible { .. }))
    }

    pub fn into_config_for_cli(self) -> Result<Config, ConfigError> {
        match self.outcome {
            Ok(ResolveOutcome::Selected { path, .. }) => {
                // Full CLI load: schema + SSH key existence (daemon start paths).
                let contents = std::fs::read_to_string(&path)?;
                Config::from_toml(&contents)
            }
            Ok(ResolveOutcome::FreshEligible { .. }) => {
                let mut config = Config::default();
                config.expand_paths();
                Ok(config)
            }
            Ok(ResolveOutcome::Conflict { candidates }) => Err(ConfigError::ConfigConflict {
                paths: candidates.into_iter().map(|c| c.path).collect(),
            }),
            Err(issue) => match issue.code {
                ResolveErrorCode::MissingExplicitConfig => {
                    Err(ConfigError::MissingExplicitConfig {
                        path: issue.path.unwrap_or_default(),
                    })
                }
                ResolveErrorCode::ConfigConflict => Err(ConfigError::ConfigConflict {
                    paths: self.candidates.into_iter().map(|c| c.path).collect(),
                }),
                _ => Err(ConfigError::Resolution(issue.message)),
            },
        }
    }
}

/// Fixture-friendly path roots for resolution (no process HOME mutation).
#[derive(Debug, Clone)]
pub struct ResolvePaths {
    pub home: PathBuf,
    pub xdg_config_home: Option<PathBuf>,
    /// When `Some`, `CLIPSYNC_CONFIG` is treated as set to this path.
    pub clipsync_config: Option<PathBuf>,
}

impl ResolvePaths {
    /// Production roots from process environment and `dirs` home.
    pub fn from_process_env() -> Result<Self, ConfigError> {
        let home = dirs::home_dir().ok_or_else(|| {
            ConfigError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not determine home directory",
            ))
        })?;
        Ok(Self {
            home,
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            clipsync_config: std::env::var_os("CLIPSYNC_CONFIG").map(PathBuf::from),
        })
    }

    /// Explicit fixture roots for tests (does not touch process env).
    pub fn for_fixture(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        Self {
            xdg_config_home: Some(home.join(".config")),
            clipsync_config: None,
            home,
        }
    }

    pub fn with_xdg_config_home(mut self, path: Option<PathBuf>) -> Self {
        self.xdg_config_home = path;
        self
    }

    pub fn with_clipsync_config(mut self, path: Option<PathBuf>) -> Self {
        self.clipsync_config = path;
        self
    }

    pub fn canonical_xdg_config_dir(&self) -> PathBuf {
        self.xdg_config_home
            .clone()
            .unwrap_or_else(|| self.home.join(".config"))
            .join("clipsync")
    }

    pub fn canonical_xdg_config_toml(&self) -> PathBuf {
        self.canonical_xdg_config_dir().join("config.toml")
    }

    pub fn legacy_mac_config_toml(&self) -> PathBuf {
        self.home
            .join("Library/Application Support/clipsync/config.toml")
    }

    /// `dirs::config_dir()`-style candidate under this home overlay.
    pub fn dirs_config_toml(&self) -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            if self.xdg_config_home.is_some() {
                return self.canonical_xdg_config_toml();
            }
            self.legacy_mac_config_toml()
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.canonical_xdg_config_toml()
        }
    }
}

/// Resolve configuration location per D02. Never writes.
pub fn resolve_config(
    paths: &ResolvePaths,
    explicit: Option<(PathBuf, ExplicitKind)>,
) -> ConfigResolution {
    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    if let Some((ref path, kind)) = explicit {
        return resolve_explicit(path, kind.provenance(), warnings, errors);
    }

    if let Some(ref env_path) = paths.clipsync_config {
        return resolve_explicit(
            env_path,
            ConfigProvenance::EnvClipSyncConfig,
            warnings,
            errors,
        );
    }

    let discovery = discover_candidates(paths);
    if !discovery.unusable.is_empty() {
        // Existing but unusable candidates block FreshEligible and silent fallback.
        errors.extend(discovery.unusable.clone());
        let issue = discovery.unusable[0].clone();
        return ConfigResolution {
            outcome: Err(issue),
            candidates: discovery.usable,
            warnings,
            errors,
        };
    }

    let candidates = discovery.usable;
    if candidates.is_empty() {
        return ConfigResolution {
            outcome: Ok(ResolveOutcome::FreshEligible {
                proposed_path: paths.canonical_xdg_config_toml(),
            }),
            candidates,
            warnings,
            errors,
        };
    }

    if candidates.len() == 1 {
        return select_candidate(candidates, 0, warnings, errors);
    }

    // Two+ candidates: identical content → prefer CanonicalXdg; else conflict.
    let all_same = candidates
        .windows(2)
        .all(|w| w[0].fingerprint.same_content(&w[1].fingerprint));
    if all_same {
        let idx = candidates
            .iter()
            .position(|c| c.provenance == ConfigProvenance::CanonicalXdg)
            .unwrap_or(0);
        warnings.push(ResolveIssue {
            code: ResolveErrorCode::ConfigConflict,
            message: "DuplicateIdenticalConfigs: multiple identical config files found; selected canonical XDG path"
                .into(),
            path: Some(candidates[idx].path.clone()),
        });
        return select_candidate(candidates, idx, warnings, errors);
    }

    // Structured conflict outcome for GUI chooser; CLI maps via into_config_for_cli.
    errors.push(ResolveIssue {
        code: ResolveErrorCode::ConfigConflict,
        message: format!(
            "ConfigConflict: {} distinct candidates; choose one before continuing",
            candidates.len()
        ),
        path: None,
    });
    ConfigResolution {
        outcome: Ok(ResolveOutcome::Conflict {
            candidates: candidates.clone(),
        }),
        candidates,
        warnings,
        errors,
    }
}

fn resolve_explicit(
    path: &Path,
    provenance: ConfigProvenance,
    mut warnings: Vec<ResolveIssue>,
    mut errors: Vec<ResolveIssue>,
) -> ConfigResolution {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => {
            match load_selected(path, provenance, &mut warnings) {
                Ok((config, fingerprint)) => {
                    let candidate = ConfigCandidate {
                        path: path.to_path_buf(),
                        provenance,
                        fingerprint,
                    };
                    ConfigResolution {
                        outcome: Ok(ResolveOutcome::Selected {
                            path: path.to_path_buf(),
                            provenance,
                            config,
                        }),
                        candidates: vec![candidate],
                        warnings,
                        errors,
                    }
                }
                Err(issue) => {
                    errors.push(issue.clone());
                    ConfigResolution {
                        outcome: Err(issue),
                        candidates: vec![],
                        warnings,
                        errors,
                    }
                }
            }
        }
        Ok(_) => {
            let issue = ResolveIssue {
                code: ResolveErrorCode::MissingExplicitConfig,
                message: format!(
                    "Explicit config path is not a regular file: {}",
                    path.display()
                ),
                path: Some(path.to_path_buf()),
            };
            errors.push(issue.clone());
            ConfigResolution {
                outcome: Err(issue),
                candidates: vec![],
                warnings,
                errors,
            }
        }
        Err(_) => {
            let issue = ResolveIssue {
                code: ResolveErrorCode::MissingExplicitConfig,
                message: format!(
                    "Explicit config path missing or unreadable: {}",
                    path.display()
                ),
                path: Some(path.to_path_buf()),
            };
            errors.push(issue.clone());
            ConfigResolution {
                outcome: Err(issue),
                candidates: vec![],
                warnings,
                errors,
            }
        }
    }
}

struct CandidateDiscovery {
    usable: Vec<ConfigCandidate>,
    unusable: Vec<ResolveIssue>,
}

fn discover_candidates(paths: &ResolvePaths) -> CandidateDiscovery {
    let mut ordered: Vec<(PathBuf, ConfigProvenance)> = Vec::new();
    ordered.push((
        paths.canonical_xdg_config_toml(),
        ConfigProvenance::CanonicalXdg,
    ));
    ordered.push((
        paths.legacy_mac_config_toml(),
        ConfigProvenance::LegacyMacAppSupport,
    ));

    let dirs_path = paths.dirs_config_toml();
    if !ordered.iter().any(|(p, _)| p == &dirs_path) {
        ordered.push((dirs_path, ConfigProvenance::DirsConfigDir));
    }

    let mut usable = Vec::new();
    let mut unusable = Vec::new();
    let mut seen_paths = Vec::new();
    for (path, provenance) in ordered {
        if seen_paths.iter().any(|p: &PathBuf| p == &path) {
            continue;
        }
        seen_paths.push(path.clone());
        match probe_candidate(&path, provenance) {
            CandidateProbe::Absent => {}
            CandidateProbe::Usable(candidate) => usable.push(candidate),
            CandidateProbe::Unusable(issue) => unusable.push(issue),
        }
    }
    CandidateDiscovery { usable, unusable }
}

enum CandidateProbe {
    Absent,
    Usable(ConfigCandidate),
    Unusable(ResolveIssue),
}

fn probe_candidate(path: &Path, provenance: ConfigProvenance) -> CandidateProbe {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CandidateProbe::Absent,
        Err(e) => CandidateProbe::Unusable(ResolveIssue {
            code: ResolveErrorCode::ConfigUnusable,
            message: format!(
                "Config candidate unreadable ({:?}): {}: {}",
                provenance,
                path.display(),
                e
            ),
            path: Some(path.to_path_buf()),
        }),
        Ok(meta) if meta.file_type().is_symlink() => CandidateProbe::Unusable(ResolveIssue {
            code: ResolveErrorCode::ConfigUnusable,
            message: format!(
                "Config candidate is a symlink ({:?}): {}",
                provenance,
                path.display()
            ),
            path: Some(path.to_path_buf()),
        }),
        Ok(meta) if !meta.file_type().is_file() => CandidateProbe::Unusable(ResolveIssue {
            code: ResolveErrorCode::ConfigUnusable,
            message: format!(
                "Config candidate is not a regular file ({:?}): {}",
                provenance,
                path.display()
            ),
            path: Some(path.to_path_buf()),
        }),
        Ok(_) => match ContentFingerprint::of_file(path) {
            Ok(fingerprint) => CandidateProbe::Usable(ConfigCandidate {
                path: path.to_path_buf(),
                provenance,
                fingerprint,
            }),
            Err(e) => CandidateProbe::Unusable(ResolveIssue {
                code: ResolveErrorCode::ConfigUnusable,
                message: format!(
                    "Config candidate unreadable ({:?}): {}: {}",
                    provenance,
                    path.display(),
                    e
                ),
                path: Some(path.to_path_buf()),
            }),
        },
    }
}

fn select_candidate(
    candidates: Vec<ConfigCandidate>,
    idx: usize,
    mut warnings: Vec<ResolveIssue>,
    mut errors: Vec<ResolveIssue>,
) -> ConfigResolution {
    let selected = candidates[idx].clone();
    match load_selected(&selected.path, selected.provenance, &mut warnings) {
        Ok((config, _)) => ConfigResolution {
            outcome: Ok(ResolveOutcome::Selected {
                path: selected.path.clone(),
                provenance: selected.provenance,
                config,
            }),
            candidates,
            warnings,
            errors,
        },
        Err(issue) => {
            errors.push(issue.clone());
            ConfigResolution {
                outcome: Err(issue),
                candidates,
                warnings,
                errors,
            }
        }
    }
}

fn load_selected(
    path: &Path,
    _provenance: ConfigProvenance,
    warnings: &mut Vec<ResolveIssue>,
) -> Result<(Config, ContentFingerprint), ResolveIssue> {
    let fingerprint = ContentFingerprint::of_file(path).map_err(|e| ResolveIssue {
        code: ResolveErrorCode::Io,
        message: e.to_string(),
        path: Some(path.to_path_buf()),
    })?;
    let contents = fs::read_to_string(path).map_err(|e| ResolveIssue {
        code: ResolveErrorCode::Io,
        message: e.to_string(),
        path: Some(path.to_path_buf()),
    })?;
    let config = Config::parse_for_resolve(&contents).map_err(|e| {
        let code = match &e {
            ConfigError::Toml(_) => ResolveErrorCode::ConfigParse,
            ConfigError::Validation(_) => ResolveErrorCode::ConfigValidation,
            _ => ResolveErrorCode::ConfigParse,
        };
        ResolveIssue {
            code,
            message: e.to_string(),
            path: Some(path.to_path_buf()),
        }
    })?;
    soft_inspect(&config, warnings);
    Ok((config, fingerprint))
}

fn soft_inspect(config: &Config, warnings: &mut Vec<ResolveIssue>) {
    if !config.auth.ssh_key.exists() {
        warnings.push(ResolveIssue {
            code: ResolveErrorCode::IdentityMissing,
            message: format!("SSH key not found at {}", config.auth.ssh_key.display()),
            path: Some(config.auth.ssh_key.clone()),
        });
    } else if let Err(e) = fs::metadata(&config.auth.ssh_key) {
        warnings.push(ResolveIssue {
            code: ResolveErrorCode::IdentityUnreadable,
            message: e.to_string(),
            path: Some(config.auth.ssh_key.clone()),
        });
    }

    if is_general_ssh_authorized_keys(&config.auth.authorized_keys) {
        warnings.push(ResolveIssue {
            code: ResolveErrorCode::AuthFileIsGeneralSsh,
            message: "authorized_keys path points at general SSH authorized_keys".into(),
            path: Some(config.auth.authorized_keys.clone()),
        });
    } else if !config.auth.authorized_keys.exists() {
        warnings.push(ResolveIssue {
            code: ResolveErrorCode::AuthFileMissing,
            message: "ClipSync authorization file missing".into(),
            path: Some(config.auth.authorized_keys.clone()),
        });
    }

    let history_db = &config.clipboard.history_db;
    if history_db.exists() {
        if let Some(ref key_path) = config.clipboard.history_key {
            if !key_path.exists() {
                warnings.push(ResolveIssue {
                    code: ResolveErrorCode::HistoryKeyMissingForExistingDb,
                    message: "History DB exists but configured history key is missing".into(),
                    path: Some(key_path.clone()),
                });
            }
        }
    }
}

/// True when path is the general OpenSSH authorized_keys file (not ClipSync-only).
pub fn is_general_ssh_authorized_keys(path: &Path) -> bool {
    let expanded = expand_tilde(path);
    if let Some(name) = expanded.file_name().and_then(|s| s.to_str()) {
        if name == "authorized_keys" {
            if let Some(parent) = expanded.parent() {
                if parent.file_name().and_then(|s| s.to_str()) == Some(".ssh") {
                    return true;
                }
            }
        }
    }
    false
}

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    PathBuf::from(shellexpand::tilde(s.as_ref()).into_owned())
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn fresh_eligible_when_empty() {
        let tmp = TempDir::new().unwrap();
        let paths = ResolvePaths::for_fixture(tmp.path());
        let res = resolve_config(&paths, None);
        assert!(res.is_fresh_eligible());
    }

    #[test]
    fn missing_explicit_hard_fails() {
        let tmp = TempDir::new().unwrap();
        let paths = ResolvePaths::for_fixture(tmp.path());
        let missing = tmp.path().join("nope.toml");
        let res = resolve_config(&paths, Some((missing.clone(), ExplicitKind::Flag)));
        match res.outcome {
            Err(issue) => {
                assert_eq!(issue.code, ResolveErrorCode::MissingExplicitConfig);
                assert_eq!(issue.path.as_deref(), Some(missing.as_path()));
            }
            Ok(_) => panic!("expected hard fail"),
        }
    }

    #[test]
    fn conflict_when_fingerprints_differ() {
        let tmp = TempDir::new().unwrap();
        let paths = ResolvePaths::for_fixture(tmp.path())
            .with_xdg_config_home(Some(tmp.path().join(".config")));
        let xdg = paths.canonical_xdg_config_toml();
        let legacy = paths.legacy_mac_config_toml();
        fs::create_dir_all(xdg.parent().unwrap()).unwrap();
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&xdg, "listen_addr = \"0.0.0.0:8484\"\n").unwrap();
        fs::write(&legacy, "listen_addr = \"0.0.0.0:9999\"\n").unwrap();
        let res = resolve_config(&paths, None);
        match res.outcome {
            Ok(ResolveOutcome::Conflict { candidates }) => {
                assert!(candidates.len() >= 2);
            }
            other => panic!("expected conflict, got {:?}", other),
        }
    }

    #[test]
    fn symlink_canonical_blocks_fresh_eligible() {
        let tmp = TempDir::new().unwrap();
        let paths = ResolvePaths::for_fixture(tmp.path());
        let xdg = paths.canonical_xdg_config_toml();
        fs::create_dir_all(xdg.parent().unwrap()).unwrap();
        let target = tmp.path().join("elsewhere.toml");
        fs::write(&target, "listen_addr = \"0.0.0.0:8484\"\n").unwrap();
        std::os::unix::fs::symlink(&target, &xdg).unwrap();
        let res = resolve_config(&paths, None);
        match res.outcome {
            Err(ref issue) => {
                assert_eq!(issue.code, ResolveErrorCode::ConfigUnusable);
                assert_eq!(issue.path.as_deref(), Some(xdg.as_path()));
            }
            Ok(other) => panic!("expected ConfigUnusable, got {:?}", other),
        }
        assert!(!res.is_fresh_eligible());
    }

    #[test]
    fn directory_canonical_blocks_selecting_legacy() {
        let tmp = TempDir::new().unwrap();
        let paths = ResolvePaths::for_fixture(tmp.path());
        let xdg = paths.canonical_xdg_config_toml();
        let legacy = paths.legacy_mac_config_toml();
        fs::create_dir_all(&xdg).unwrap(); // path exists as directory
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(
            &legacy,
            r#"listen_addr = "0.0.0.0:8484"
advertise_name = "x"
log_level = "info"
[auth]
ssh_key = "/tmp/k"
authorized_keys = "/tmp/a"
[clipboard]
max_size = 1048576
sync_primary = true
history_size = 20
history_db = "/tmp/h.db"
"#,
        )
        .unwrap();
        let res = resolve_config(&paths, None);
        match res.outcome {
            Err(issue) => assert_eq!(issue.code, ResolveErrorCode::ConfigUnusable),
            Ok(ResolveOutcome::Selected { path, .. }) => {
                panic!("must not silently select legacy {}", path.display())
            }
            Ok(other) => panic!("expected ConfigUnusable, got {:?}", other),
        }
    }
}
