//! Typed failures for desktop-user service operations.

use std::path::PathBuf;

/// Actionable service lifecycle errors (no silent success on failure).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    #[error("path invalid ({path}): {reason}")]
    PathInvalid { path: PathBuf, reason: String },

    #[error("managed service path mismatch: expected {expected}, got {actual}")]
    NotManagedUnit { expected: String, actual: String },

    #[error("legacy root/system clipsync service present; refuse competing user start")]
    LegacyRootConflict,

    #[error("unit/plist is not the managed ClipSync user service")]
    UnrecognizedUnit,

    #[error("host command failed: {program} {args:?} (status={status:?}): {stderr}")]
    HostCommandFailed {
        program: String,
        args: Vec<String>,
        status: Option<i32>,
        stderr: String,
        timed_out: bool,
    },

    #[error("host command timed out: {program} {args:?}")]
    TimedOut { program: String, args: Vec<String> },

    #[error("rollback failed after error `{during}`: {detail}")]
    RollbackFailed { during: String, detail: String },

    #[error("io error: {0}")]
    Io(String),

    #[error("platform unsupported for user service management")]
    UnsupportedPlatform,

    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for ServiceError {
    fn from(value: std::io::Error) -> Self {
        ServiceError::Io(value.to_string())
    }
}

impl ServiceError {
    pub fn io(context: impl Into<String>, err: std::io::Error) -> Self {
        ServiceError::Io(format!("{}: {err}", context.into()))
    }
}
