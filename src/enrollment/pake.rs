//! One-time pairing codes and the PAKE that proves both computers hold one.
//!
//! SPAKE2 over the Ed25519 group uses the six-digit code as the password and
//! fixed role identities, which `spake2` hashes into the shared key along with
//! both PAKE messages. An attacker gets one online guess per connection: the
//! allowing side consumes the code before reading the peer's PAKE message.
//!
//! Key confirmation binds that key to this TLS connection (RFC 8446 exporter)
//! and to both TLS-authenticated keys. A relay that forwards the PAKE messages
//! between two honest computers while presenting its own key to each sees two
//! different transcripts, so neither confirmation verifies.

use super::tls::ENROLL_ALPN;
use crate::control::PairingCode;
use anyhow::{anyhow, Result};
use rand::{rngs::OsRng, TryRngCore};
use ring::{hkdf, hmac};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use uuid::Uuid;
use zeroize::Zeroizing;

const ALLOWING_ID: &[u8] = b"clipsync-enroll/2 allowing";
const JOINING_ID: &[u8] = b"clipsync-enroll/2 joining";
const TRANSCRIPT_LABEL: &[u8] = b"clipsync-enroll/2 transcript";
const CODE_SPACE: u32 = 1_000_000;
/// Largest multiple of `CODE_SPACE` that fits in a u32; draws at or above it are
/// discarded so every code is equally likely.
const UNBIASED_LIMIT: u32 = u32::MAX - (u32::MAX % CODE_SPACE);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Shows the code and listens.
    Allowing,
    /// Types the code and dials.
    Joining,
}

impl Side {
    fn other(self) -> Self {
        match self {
            Self::Allowing => Self::Joining,
            Self::Joining => Self::Allowing,
        }
    }

    fn confirm_label(self) -> &'static [u8] {
        match self {
            Self::Allowing => b"clipsync-enroll/2 key confirmation from allowing",
            Self::Joining => b"clipsync-enroll/2 key confirmation from joining",
        }
    }
}

/// A uniformly random code from the operating system's CSPRNG.
pub fn generate_code() -> Result<PairingCode> {
    let mut rng = OsRng;
    loop {
        let draw = rng
            .try_next_u32()
            .map_err(|_| anyhow!("Enrollment code unavailable"))?;
        if draw < UNBIASED_LIMIT {
            let text = Zeroizing::new(format!("{:06}", draw % CODE_SPACE));
            return PairingCode::parse(&text).ok_or_else(|| anyhow!("Enrollment code unavailable"));
        }
    }
}

pub struct Pake {
    side: Side,
    state: Spake2<Ed25519Group>,
    outbound: Vec<u8>,
}

impl Pake {
    /// Takes the code by value; from here on it exists only inside the PAKE state.
    pub fn start(side: Side, code: PairingCode) -> Self {
        let password = Password::new(code.digits().as_bytes());
        drop(code);
        let (allowing, joining) = (Identity::new(ALLOWING_ID), Identity::new(JOINING_ID));
        let (state, outbound) = match side {
            Side::Allowing => Spake2::<Ed25519Group>::start_a(&password, &allowing, &joining),
            Side::Joining => Spake2::<Ed25519Group>::start_b(&password, &allowing, &joining),
        };
        Self {
            side,
            state,
            outbound,
        }
    }

    /// This side's PAKE message, to send to the peer.
    pub fn message(&self) -> &[u8] {
        &self.outbound
    }

    pub fn finish(self, peer_message: &[u8]) -> Result<SharedKey> {
        let key = self
            .state
            .finish(peer_message)
            .map_err(|_| anyhow!("Enrollment protocol"))?;
        Ok(SharedKey {
            key: Zeroizing::new(key),
            side: self.side,
        })
    }
}

/// Everything both computers must agree on. Fingerprints come from the TLS
/// session (this computer's key and the TLS peer key), never from messages.
pub struct Transcript<'a> {
    pub allowing_session: Uuid,
    pub joining_session: Uuid,
    pub allowing_nonce: &'a [u8; 32],
    pub joining_nonce: &'a [u8; 32],
    pub allowing_fingerprint: &'a str,
    pub joining_fingerprint: &'a str,
    pub allowing_message: &'a [u8],
    pub joining_message: &'a [u8],
    pub channel_binding: &'a [u8; 32],
}

fn push(out: &mut Vec<u8>, field: &[u8]) {
    out.extend_from_slice(&(field.len() as u32).to_be_bytes());
    out.extend_from_slice(field);
}

impl Transcript<'_> {
    fn mac_input(&self, from: Side) -> Vec<u8> {
        let mut out = Vec::with_capacity(512);
        for field in [
            from.confirm_label(),
            TRANSCRIPT_LABEL,
            ENROLL_ALPN,
            self.allowing_session.as_bytes(),
            self.joining_session.as_bytes(),
            self.allowing_nonce,
            self.joining_nonce,
            self.allowing_fingerprint.as_bytes(),
            self.joining_fingerprint.as_bytes(),
            self.allowing_message,
            self.joining_message,
            self.channel_binding,
        ] {
            push(&mut out, field);
        }
        out
    }
}

pub struct SharedKey {
    key: Zeroizing<Vec<u8>>,
    side: Side,
}

impl SharedKey {
    fn mac_key(&self, from: Side) -> Result<hmac::Key> {
        let info = [from.confirm_label()];
        hkdf::Salt::new(hkdf::HKDF_SHA256, ENROLL_ALPN)
            .extract(&self.key)
            .expand(&info, hmac::HMAC_SHA256)
            .map(hmac::Key::from)
            .map_err(|_| anyhow!("Enrollment protocol"))
    }

    /// This side's key confirmation over `transcript`.
    pub fn confirmation(&self, transcript: &Transcript<'_>) -> Result<Vec<u8>> {
        let key = self.mac_key(self.side)?;
        Ok(hmac::sign(&key, &transcript.mac_input(self.side))
            .as_ref()
            .to_vec())
    }

    /// Constant-time check of the peer's confirmation over the same transcript.
    pub fn verify_peer(&self, transcript: &Transcript<'_>, tag: &[u8]) -> bool {
        let peer = self.side.other();
        self.mac_key(peer)
            .is_ok_and(|key| hmac::verify(&key, &transcript.mac_input(peer), tag).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Parties {
        allowing: SharedKey,
        joining: SharedKey,
        allowing_msg: Vec<u8>,
        joining_msg: Vec<u8>,
    }

    fn run(allowing_code: &str, joining_code: &str) -> Parties {
        let a = Pake::start(Side::Allowing, PairingCode::parse(allowing_code).unwrap());
        let j = Pake::start(Side::Joining, PairingCode::parse(joining_code).unwrap());
        let (allowing_msg, joining_msg) = (a.message().to_vec(), j.message().to_vec());
        Parties {
            allowing: a.finish(&joining_msg).unwrap(),
            joining: j.finish(&allowing_msg).unwrap(),
            allowing_msg,
            joining_msg,
        }
    }

    const NONCE_A: [u8; 32] = [1; 32];
    const NONCE_J: [u8; 32] = [2; 32];
    const BINDING: [u8; 32] = [3; 32];

    fn transcript<'a>(p: &'a Parties, allowing_fp: &'a str, joining_fp: &'a str) -> Transcript<'a> {
        Transcript {
            allowing_session: Uuid::from_u128(1),
            joining_session: Uuid::from_u128(2),
            allowing_nonce: &NONCE_A,
            joining_nonce: &NONCE_J,
            allowing_fingerprint: allowing_fp,
            joining_fingerprint: joining_fp,
            allowing_message: &p.allowing_msg,
            joining_message: &p.joining_msg,
            channel_binding: &BINDING,
        }
    }

    #[test]
    fn codes_are_six_digits_and_cover_every_leading_digit() {
        let mut leading = [false; 10];
        for _ in 0..2000 {
            let code = generate_code().unwrap();
            assert_eq!(code.digits().len(), 6);
            assert!(code.digits().bytes().all(|b| b.is_ascii_digit()));
            leading[usize::from(code.digits().as_bytes()[0] - b'0')] = true;
        }
        assert!(leading.iter().all(|seen| *seen), "{leading:?}");
        assert_eq!(UNBIASED_LIMIT % CODE_SPACE, 0);
        assert!(u32::MAX - UNBIASED_LIMIT < CODE_SPACE);
    }

    #[test]
    fn matching_codes_confirm_in_both_directions_with_distinct_tags() {
        let p = run("482913", "482913");
        let t = transcript(&p, "SHA256:allow", "SHA256:join");
        let from_joining = p.joining.confirmation(&t).unwrap();
        let from_allowing = p.allowing.confirmation(&t).unwrap();
        assert!(p.allowing.verify_peer(&t, &from_joining));
        assert!(p.joining.verify_peer(&t, &from_allowing));
        assert_ne!(from_joining, from_allowing);
        // Reflecting a side's own tag back never verifies as the peer's.
        assert!(!p.allowing.verify_peer(&t, &from_allowing));
        assert!(!p.joining.verify_peer(&t, &from_joining));
    }

    #[test]
    fn a_wrong_code_fails_both_confirmations() {
        let p = run("482913", "482914");
        let t = transcript(&p, "SHA256:allow", "SHA256:join");
        assert!(!p
            .allowing
            .verify_peer(&t, &p.joining.confirmation(&t).unwrap()));
        assert!(!p
            .joining
            .verify_peer(&t, &p.allowing.confirmation(&t).unwrap()));
    }

    #[test]
    fn a_relay_presenting_its_own_keys_fails_both_confirmations() {
        // Same code, PAKE messages relayed untouched, but each honest side sees
        // the relay's TLS key as its peer.
        let p = run("482913", "482913");
        let at_allowing = transcript(&p, "SHA256:allow", "SHA256:relay-to-allowing");
        let at_joining = transcript(&p, "SHA256:relay-to-joining", "SHA256:join");
        let tag = p.joining.confirmation(&at_joining).unwrap();
        assert!(!p.allowing.verify_peer(&at_allowing, &tag));
        let tag = p.allowing.confirmation(&at_allowing).unwrap();
        assert!(!p.joining.verify_peer(&at_joining, &tag));
    }

    #[test]
    fn every_transcript_field_is_bound() {
        let p = run("000001", "000001");
        let other_nonce = [9u8; 32];
        let other_binding = [8u8; 32];
        let base = transcript(&p, "SHA256:allow", "SHA256:join");
        let tag = p.joining.confirmation(&base).unwrap();
        assert!(p.allowing.verify_peer(&base, &tag));
        let variants = [
            Transcript {
                allowing_session: Uuid::from_u128(7),
                ..transcript(&p, "SHA256:allow", "SHA256:join")
            },
            Transcript {
                joining_session: Uuid::from_u128(7),
                ..transcript(&p, "SHA256:allow", "SHA256:join")
            },
            Transcript {
                allowing_nonce: &other_nonce,
                ..transcript(&p, "SHA256:allow", "SHA256:join")
            },
            Transcript {
                joining_nonce: &other_nonce,
                ..transcript(&p, "SHA256:allow", "SHA256:join")
            },
            transcript(&p, "SHA256:other", "SHA256:join"),
            transcript(&p, "SHA256:allow", "SHA256:other"),
            Transcript {
                allowing_message: &p.joining_msg,
                ..transcript(&p, "SHA256:allow", "SHA256:join")
            },
            Transcript {
                joining_message: &p.allowing_msg,
                ..transcript(&p, "SHA256:allow", "SHA256:join")
            },
            Transcript {
                channel_binding: &other_binding,
                ..transcript(&p, "SHA256:allow", "SHA256:join")
            },
            // Length prefixes keep a byte moved across a field boundary distinct.
            transcript(&p, "SHA256:allowS", "HA256:join"),
        ];
        for (index, variant) in variants.iter().enumerate() {
            assert!(!p.allowing.verify_peer(variant, &tag), "field {index}");
        }
    }

    #[test]
    fn messages_from_the_same_role_are_refused() {
        let a1 = Pake::start(Side::Allowing, PairingCode::parse("123456").unwrap());
        let a2 = Pake::start(Side::Allowing, PairingCode::parse("123456").unwrap());
        let reflected = a2.message().to_vec();
        assert!(a1.finish(&reflected).is_err());
    }

    #[test]
    fn pairing_code_is_redacted_and_validated() {
        let code = PairingCode::parse("482 913").unwrap();
        assert_eq!(code.digits(), "482913");
        assert_eq!(code.grouped(), "482 913");
        assert!(!format!("{code:?}").contains("482"));
        assert!(!format!("{:?}", Some(code.clone())).contains("913"));
        assert_eq!(serde_json::to_string(&code).unwrap(), "\"482913\"");
        for bad in ["48291", "4829133", "48291a", "", "４８２９１３"] {
            assert!(PairingCode::parse(bad).is_none(), "{bad}");
            let err = serde_json::from_str::<PairingCode>(&format!("\"{bad}\""))
                .unwrap_err()
                .to_string();
            assert!(bad.is_empty() || !err.contains(bad), "{err}");
        }
    }
}
