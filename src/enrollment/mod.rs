//! LAN pairing with a one-time code.
//!
//! The allowing computer listens on a separate TLS 1.3 Ed25519 raw-public-key
//! channel (ALPN `clipsync-enroll/2`) and shows a six-digit code. The joining
//! computer dials and both run SPAKE2 with that code, then exchange key
//! confirmations bound to the TLS session and both TLS keys. Trust is written
//! only after the peer's confirmation verifies. Production sync (`clipsync/2`)
//! and the authorized-devices file are unchanged.
//!
//! Each connection consumes the code on screen before any PAKE input is read,
//! so a stranger gets one guess per code and a window accepts a bounded number
//! of attempts. Mutual success requires local durable pin **and** peer
//! `PersistAck`; otherwise the outcome is `waiting_peer` / `incomplete`.

#![deny(warnings, clippy::all)]

mod channel;
mod commit_gate;
mod limits;
mod manager;
mod mdns;
mod pake;
mod protocol;
mod session_io;
mod tls;

pub use limits::{
    CODE_TTL, MAX_ACCEPTS_PER_MINUTE, MAX_CONCURRENT_SESSIONS, MAX_FAILURES_PER_FINGERPRINT,
    MAX_PAIRING_ATTEMPTS, SESSION_TTL, WINDOW_TTL,
};
pub use manager::EnrollmentManager;
pub use protocol::{EnrollMessage, MAX_FRAME_BYTES as ENROLL_MAX_FRAME_BYTES};
pub use tls::{enroll_configs, enroll_configs_with, ENROLL_ALPN, LEGACY_ENROLL_ALPN};
