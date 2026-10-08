# Security model after the transport repair

The daemon uses WebSocket framing over **mutually authenticated TLS 1.3**, implemented by rustls. RFC 7250 raw public keys reuse the configured Ed25519 identity. Each side pins the peer public key from its ClipSync `authorized_keys` file; rustls verifies private-key possession through the TLS handshake signature and protects application records. No custom key exchange is implemented.

Only Ed25519 identities are supported. Real OpenSSH public-key wire encoding and the previous ClipSync raw-32-byte encoding are accepted; newly written public keys use OpenSSH encoding. Displayed fingerprints now match `ssh-keygen`. Existing authorized keys remain usable; node IDs are derived from keys instead of ephemeral configuration UUIDs.

Discovery announces public information and supplies address hints. It cannot authorize a peer. Both peers must explicitly add each other's public key after verification through a trusted channel. The outbound connection additionally checks that the TLS-authenticated key matches the discovered node ID. Old interactive discovery trust caches do not bypass this check.

TLS 1.2, plaintext, missing/untrusted identities, and wrong signing keys fail closed. Session resumption and early data are disabled so each new connection checks authorization. Existing connections recheck authorization before clipboard traffic and on their heartbeat; revocation takes effect at the next such check. Unknown peers receive no clipboard payload.

Protocol 2.0.0 deliberately breaks compatibility with the old plaintext protocol. Upgrade both devices. The transport chooses its cipher through TLS; the old `security.encryption` setting does not choose a wire cipher. The repaired text path does not compress network payloads.

Local clipboard-history **contents** use AES-256-GCM. The SQLite container and metadata (timestamps, content types/sizes, checksums and origin IDs) are not fully encrypted. The SQLite SQLCipher build dependency alone does not imply database-wide encryption. Keys default to the existing platform configuration location under `clipsync/history.key`; `clipboard.history_key` permits an explicit location for an isolated store. Existing key-load errors are fatal and do not trigger replacement. New keys are published atomically with mode 0600. Missing keys alongside an existing database require recovery from backup.

The local control socket is placed inside an owner-only directory, checks Unix peer credentials at both ends, bounds frame size and request duration, and uses an advisory lock for instance ownership. A process running as the same OS user remains inside the trust boundary.

Event order uses a millisecond-based logical clock, then authenticated origin ID and event UUID as deterministic tie-breakers. Peers more than five minutes ahead are rejected. Duplicate/older events are ignored; applying a remote value updates monitor state to prevent an echo. This is clipboard convergence, not a durable message-delivery protocol. `sync` reports queue acceptance. Only the most recent local event is retained in memory for reconnect, and process restart does not restore an outgoing queue.

The sensitive-content filter is heuristic. It can miss secrets or suppress harmless strings. A trusted peer can write clipboard content by design. Compromised same-user processes and clipboard consumers are outside the protection offered by transport encryption.

See [REPAIR_VALIDATION.md](REPAIR_VALIDATION.md) for tests performed. This change has not received an independent cryptographic review or a release/package signing audit.
