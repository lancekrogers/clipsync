//! Enrollment channel messages. No clipboard / sync payloads.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// Bound for each handshake read/write; nobody waits on a person after the code is typed.
pub const IO_TIMEOUT_SECS: u64 = 15;
/// Once a session frame length byte arrives, bound the remainder.
pub const PARTIAL_FRAME_TIMEOUT_SECS: u64 = 15;
/// Bound for waiting on peer durable PersistAck after local pin.
pub const PERSIST_ACK_TIMEOUT_SECS: u64 = 10;
pub const MAX_DISPLAY_NAME_CHARS: usize = 64;

/// Stable `Error.code` values the allowing side sends before closing.
pub mod refusal {
    pub const NO_CODE: &str = "no_code";
    pub const WRONG_CODE: &str = "wrong_code";
    pub const BUSY: &str = "busy";
    pub const RATE_LIMITED: &str = "rate_limited";
    pub const REFLECTION: &str = "reflection";
    pub const CLOSED: &str = "closed";
    pub const PROTOCOL: &str = "protocol";
    /// Sent on `clipsync-enroll/1`; older dialers show `message` as the session error.
    pub const UPDATE_REQUIRED: &str = "update_required";
}

/// `clipsync-enroll/2` frames. Joining sends Hello and the first KeyConfirm; the
/// allowing side answers only after it has consumed its code. Fingerprints are
/// never sent: each side reads the other's key from the TLS session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EnrollMessage {
    Hello {
        session_id: Uuid,
        /// 32-byte nonce, hex-encoded
        nonce_hex: String,
        display_name: String,
        /// SPAKE2 message from the joining side.
        pake_hex: String,
    },
    HelloAck {
        session_id: Uuid,
        peer_session_id: Uuid,
        nonce_hex: String,
        display_name: String,
        /// SPAKE2 message from the allowing side.
        pake_hex: String,
    },
    /// HMAC under the PAKE key over the transcript; joining sends first.
    KeyConfirm {
        session_id: Uuid,
        peer_session_id: Uuid,
        mac_hex: String,
    },
    /// Honest durable-commit signal: sender persisted the peer's key.
    PersistAck {
        session_id: Uuid,
        peer_session_id: Uuid,
        /// Fingerprint of the key the sender just authorized (peer's key).
        authorized_fingerprint: String,
    },
    Cancel {
        session_id: Uuid,
    },
    Error {
        code: String,
        message: String,
    },
}

pub fn random_nonce() -> [u8; 32] {
    use rand::RngCore;
    let mut n = [0u8; 32];
    rand::rng().fill_bytes(&mut n);
    n
}

/// Sanitize untrusted peer/local display labels for UI and authorized_keys comments.
/// Rejects CR/LF/NUL (authorization-line injection) and bounds length.
pub fn sanitize_display_name(raw: &str) -> anyhow::Result<String> {
    if raw.bytes().any(|b| b == b'\n' || b == b'\r' || b == 0) {
        anyhow::bail!("Enrollment mismatch");
    }
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DISPLAY_NAME_CHARS)
        .collect();
    if cleaned.is_empty() {
        Ok("device".into())
    } else {
        Ok(cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_newline_key_injection() {
        let evil = "friend\nssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl x";
        assert!(sanitize_display_name(evil).is_err());
    }

    #[test]
    fn sanitize_bounds_length() {
        let long = "a".repeat(200);
        let out = sanitize_display_name(&long).unwrap();
        assert_eq!(out.len(), MAX_DISPLAY_NAME_CHARS);
    }
}
