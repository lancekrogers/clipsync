# Security model after the transport repair

The daemon uses WebSocket framing over **mutually authenticated TLS 1.3**, implemented by rustls. RFC 7250 raw public keys reuse the configured Ed25519 identity. Each side pins the peer public key from its ClipSync `authorized_keys` file; rustls verifies private-key possession through the TLS handshake signature and protects application records. No custom key exchange is implemented.

Only Ed25519 identities are supported. Real OpenSSH public-key wire encoding and the previous ClipSync raw-32-byte encoding are accepted; newly written public keys use OpenSSH encoding. Displayed fingerprints now match `ssh-keygen`. Existing authorized keys remain usable; node IDs are derived from keys instead of ephemeral configuration UUIDs.

Discovery announces public information and supplies address hints. It cannot authorize a peer. A key is added to the authorized-devices file only by [pairing with a code](#pairing-a-new-device) or by `clipsync auth add` after checking the fingerprint out of band. The outbound connection additionally checks that the TLS-authenticated key matches the discovered node ID. Old interactive discovery trust caches do not bypass this check.

TLS 1.2, plaintext, missing/untrusted identities, and wrong signing keys fail closed. Session resumption and early data are disabled so each new connection checks authorization. Existing connections recheck authorization before clipboard traffic and on their heartbeat; revocation takes effect at the next such check. Unknown peers receive no clipboard payload.

Protocol 2.0.0 deliberately breaks compatibility with the old plaintext protocol. Upgrade both devices. The transport chooses its cipher through TLS; the old `security.encryption` setting does not choose a wire cipher. The repaired text path does not compress network payloads.

Local clipboard-history **contents** use AES-256-GCM. The SQLite container and metadata (timestamps, content types/sizes, checksums and origin IDs) are not fully encrypted. The SQLite SQLCipher build dependency alone does not imply database-wide encryption. Keys default to the existing platform configuration location under `clipsync/history.key`; `clipboard.history_key` permits an explicit location for an isolated store. Existing key-load errors are fatal and do not trigger replacement. New keys are published atomically with mode 0600. Missing keys alongside an existing database require recovery from backup.

The local control socket is placed inside an owner-only directory, checks Unix peer credentials at both ends, bounds frame size and request duration, and uses an advisory lock for instance ownership. A process running as the same OS user remains inside the trust boundary.

Event order uses a millisecond-based logical clock, then authenticated origin ID and event UUID as deterministic tie-breakers. Peers more than five minutes ahead are rejected. Duplicate/older events are ignored; applying a remote value updates monitor state to prevent an echo. This is clipboard convergence, not a durable message-delivery protocol. `sync` reports queue acceptance. Only the most recent local event is retained in memory for reconnect, and process restart does not restore an outgoing queue.

The sensitive-content filter is heuristic. It can miss secrets or suppress harmless strings. A trusted peer can write clipboard content by design. Compromised same-user processes and clipboard consumers are outside the protection offered by transport encryption.

## Pairing a new device

Pairing runs on its own TLS 1.3 raw-public-key listener (ALPN `clipsync-enroll/2`), separate from sync, and only while someone has chosen **Allow a new device**. That computer shows a six-digit code drawn uniformly from the operating system's random generator. The code reaches only the local companion over the owner-only control socket; it is kept out of logs, tracing, the diagnostics export, mDNS records, error text and `Debug` output.

The joining computer dials, and both run SPAKE2 over the Ed25519 group with the code as the password and fixed, distinct role identities; the `spake2` crate hashes both identities and both PAKE messages into the shared key. Each side then sends an HMAC-SHA256 key confirmation, under a per-direction key derived with HKDF, over a transcript of: direction label, protocol version, both session IDs and nonces, both PAKE messages, both key fingerprints as observed in the TLS session (never as claimed in a message), and the TLS exporter for this connection. The joining side confirms first and the allowing side verifies before sending anything keyed. Each side writes trust only after the other's confirmation verifies. Mutual success still requires both durable-save acknowledgements; otherwise the result is reported as waiting or incomplete.

Each connection to the listener consumes the code on screen before the peer's PAKE message is read, and a fresh code appears. Codes also renew every two minutes. One window accepts five attempts and closes after ten minutes, when its attempts are used, or once a device has paired.

**A stranger on the same network can:**

- see, while a window is open, that the computer is allowing a device (its name, node ID and key fingerprint in an mDNS announcement) and connect to it;
- guess once per code: at most five guesses per window, a chance of at most 5 in 1,000,000 that a window a person opened pairs a stranger. A wrong guess is shown on the allowing screen and spends one of its codes;
- deny service by spending codes, holding the three concurrent attempt slots, or using up a window's attempts. The person then opens a new window; nothing is paired;
- announce a look-alike name under **Find nearby**. Picking it pairs nothing: the code exists only on the real computer's screen, and a stranger who receives a joining computer's PAKE message still has one online guess and no offline test.

**A stranger on the same network cannot:**

- pair without typing the code shown on the allowing screen, correctly, on that code's single attempt;
- learn the code from the traffic: the exchange is inside TLS, and SPAKE2 messages reveal nothing that can be checked offline;
- relay a pairing between two honest computers while presenting its own keys: each side binds the fingerprints it saw in TLS and the TLS exporter, so both confirmations fail;
- reuse a code that was already tried, or replay an earlier exchange (fresh session IDs, nonces, PAKE secrets and TLS exporter each time);
- pair with the old fingerprint-compare protocol: `clipsync-enroll/1` is only negotiated to tell an older computer to update.

**Trade-offs.** The `spake2` crate (RustCrypto, 0.4) has never received an independent third-party audit; its README says so. Using it reverses an earlier decision against PAKE crates, accepted so that pairing cannot be approved without entering the code (the previous fingerprint comparison could be skipped by pressing `y`). The burn, transcript binding and key confirmation are ClipSync's own code, covered by tests but not externally reviewed. Anyone who can see the allowing screen while it shows a code, and reaches the listener first, can pair; only allow a device when you are about to type the code. A process running as the same OS user can read the code through the control socket, as it can read everything else the daemon exposes locally.

See [REPAIR_VALIDATION.md](REPAIR_VALIDATION.md) for tests performed. This change has not received an independent cryptographic review or a release/package signing audit.
