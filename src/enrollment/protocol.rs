//! Enrollment channel messages. No clipboard / sync payloads.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// Bound for handshake and write stalls (not human verify idle).
pub const IO_TIMEOUT_SECS: u64 = 15;
/// Once a session frame length byte arrives, bound the remainder.
pub const PARTIAL_FRAME_TIMEOUT_SECS: u64 = 15;
/// Bound for waiting on peer durable PersistAck after local pin.
pub const PERSIST_ACK_TIMEOUT_SECS: u64 = 10;
pub const MAX_DISPLAY_NAME_CHARS: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EnrollMessage {
    Hello {
        session_id: Uuid,
        /// 32-byte nonce, hex-encoded
        nonce_hex: String,
        local_fingerprint: String,
        display_name: String,
    },
    HelloAck {
        session_id: Uuid,
        peer_session_id: Uuid,
        nonce_hex: String,
        local_fingerprint: String,
        display_name: String,
    },
    Confirm {
        session_id: Uuid,
        peer_session_id: Uuid,
        /// Bind: local nonce || peer nonce || both fingerprints
        binding_hex: String,
        confirmed: bool,
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

pub fn binding_hex(local_nonce: &[u8], peer_nonce: &[u8], local_fp: &str, peer_fp: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(local_nonce);
    hasher.update(peer_nonce);
    hasher.update(local_fp.as_bytes());
    hasher.update(peer_fp.as_bytes());
    hex::encode(hasher.finalize())
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
