# ClipSync repair validation

The repair replaces unfinished daemon transport operations with authenticated TLS connections, fixes key preservation and event admission, provides Unix-socket daemon control, and corrects discovery and clipboard integration. The older documentation under `ai_docs/` describes earlier plans and is not evidence of implemented behavior.

## Executable evidence

- `tests/secure_sync.rs`: encrypted bidirectional loopback delivery; no remote echo; reconnection (including a copy made while disconnected); live revocation; stale/duplicate rejection; failed-write retry; excessive clock skew rejection; wrong-server rejection; unknown-key and public-key-only impersonation rejection; plaintext rejection; captured traffic lacks the clipboard fixture and its JSON message type.
- `tests/history_recovery.rs`: existing key/database byte preservation on insecure permissions, malformed key, and missing-key conditions; successful reopen with the original key; competing initializers converge on one complete key.
- `tests/daemon_control.rs`: actual separate CLI processes contact a control server; duplicate daemon binding fails; oversized requests are rejected without stopping the server; no-daemon requests exit unsuccessfully.
- `src/clipboard/macos.rs`: native read/write/clear roundtrip on two unique named NSPasteboards, leaving the general clipboard alone. This caught incorrect use of constant names as pasteboard types.
- `scripts/test/desktop_sync.py`: two independent daemon processes, standard `ssh-keygen` identities, non-default ports, automatic discovery, bidirectional real clipboard changes, separate status/peers/sync/history commands, SIGTERM shutdown, history reopen and restart/reconnect. Runs under both Xvfb and headless Sway in a disposable Linux container.
- Existing concurrent discovery tests now complete after replacing a blocking receive with asynchronous receive. Event subscribers are installed synchronously; slow listeners cannot block every subscriber, and forwarding tasks stop with discovery.

`just test-desktop` runs the Linux suite and both desktop scenarios. Docker is required. `just check` and `just test` are the regular project gates. Formatting was normalized to make the pre-existing formatting gate runnable. Strict warning/Clippy checks are enabled on the rewritten sync, transport and control modules; legacy library-wide suppressions remain elsewhere.

## Connection recovery and isolated physical testing

`tests/connection_recovery.rs` exercises discovery-driven dialing in both node-ID
orderings, simultaneous connections, cancellation/retry, and a stalled first TLS
endpoint followed by a reachable alternative. Discovery advertises addresses
compatible with the listener and replaces stale address snapshots.

For an explicitly isolated Mac pilot, build with `--features integration-tests`
and set `CLIPSYNC_TEST_PASTEBOARD` to a unique `org.clipsync.test.*` name. This
selects a private native pasteboard. Normal builds reject that environment
variable, and the test feature rejects names outside that prefix. Never point
acceptance runs at the general clipboard. Pair dedicated temporary SSH identities,
use separate history/config paths, and release the named pasteboard at cleanup.

## Limits

The tested deployment path is foreground operation and graceful termination. Headless Sway establishes data-control behavior, not compatibility with every Wayland compositor. macOS clipboard integration was tested with named pasteboards; no user LaunchAgent was installed and no personal clipboard was changed. An isolated physical Mac/KWin LAN retest on 2026-10-09 passed in both node-ID orderings with Linux inbound connections blocked, including bidirectional transfers and graceful/crash recovery on both hosts. This used the connection and daemon-owned-copy fixes together, private clipboards and temporary identities; installed host services and firewall rules were unchanged. Sleep/wake, graphical login/logout and installed-service migration still need release acceptance. One pre-existing mock-channel test remains ignored; on Linux, the test that writes to the ambient X11 clipboard is also ignored in favor of the isolated desktop harness.

Clipboard synchronization is text-only. Delivery is queued rather than acknowledged as applied; history metadata is not encrypted. A dependency emits a future Rust compatibility warning (`proc-macro-error2`); this is not a test failure. No broad dependency upgrade or security-advisory audit was performed.
