//! Secure LAN enrollment (D03 / PAIRING_THREAT_MODEL).
//!
//! Enrollment uses a separate TLS 1.3 Ed25519 raw-public-key channel with ALPN
//! `clipsync-enroll/1`. Production sync (`clipsync/2`) is unchanged. Discovery
//! hints never authorize; both sides must Open and Confirm full fingerprints
//! derived from the TLS-authenticated peer key before any trust write.
//!
//! Session TLS I/O uses an actor with split read/write halves (no duplex Mutex
//! across blocking reads). Mutual success requires local durable pin **and**
//! peer `PersistAck`; otherwise outcome is `waiting_peer` / `incomplete`.

#![deny(warnings, clippy::all)]

mod channel;
mod commit_gate;
mod limits;
mod manager;
mod mdns;
mod protocol;
mod session_io;
mod tls;

pub use limits::{
    MAX_ACCEPTS_PER_MINUTE, MAX_CONCURRENT_SESSIONS, MAX_FAILURES_PER_FINGERPRINT, SESSION_TTL,
};
pub use manager::EnrollmentManager;
pub use protocol::{EnrollMessage, MAX_FRAME_BYTES as ENROLL_MAX_FRAME_BYTES};
pub use tls::{enroll_configs, ENROLL_ALPN};
