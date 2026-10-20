# Companion verification

The desktop and terminal companions are experimental. This change adds the
interfaces and their shared daemon control, setup, service and enrollment paths.
Public distribution and physical desktop lifecycle acceptance remain separate
release gates.

## Reproduce the checks

Use Rust 1.95 for the workspace. The original daemon remains the default member.

```sh
just companion-verify
just companion-test-macos
just companion-test-linux
just companion-package-test
just companion-package
```

The macOS UI runner needs `uv`, Swift, `cliclick` and already-enabled Accessibility
for System Events. It uses dedicated named pasteboards, temporary identities and
configuration directories. It never installs a service. It opens real native
windows and restores the terminal after the PTY checks. Close previous acceptance
windows before running it; the inspection ports are reserved by the runner.

Linux GUI checks run under Xvfb and headless Sway in a disposable container, using
its clipboard and filesystem. The source mount is read-only. Service lifecycle
checks inject launchd/systemd command results; they do not change host services.

`ui-test` and `integration-tests` are acceptance-only features. Packaging builds
omit both. Production companions do not expose the UI inspection server or use
test pasteboards.

## Evidence covered by the harnesses

| Area | Observable behavior |
|---|---|
| History | Real encrypted rows, search and empty states, UUID-bound details, original text copied through daemon IPC, Unicode pagination |
| Reconciliation | Restart identity and generation ordering, disconnected action gating, rapid selection changes, old-daemon capability mismatch |
| Desktop | Native window, minimum-size action visibility, keyboard focus, first-run files, tray close/reopen/pause/quit on macOS |
| Terminal | Shared routes, search, copy, pause, narrow terminal, NO_COLOR, terminal restoration |
| Setup | Explicit/legacy/conflicting configurations, no-clobber identity creation, private permissions, repeat/interrupted setup, existing history key preservation |
| Service | Per-user unit rendering, selected-config matching, startup preference, failure propagation and rollback with injected command hosts |
| Authorization | Concurrent real CLI updates, immediate add/revoke visibility, malicious comment rejection, mutual full-fingerprint confirmation |
| Packaging | Matching CLI/GUI, manifests and hashes, Linux desktop argument escaping, pair replacement rollback, explicit unsigned state, mocked notarization ordering |

Enrollment uses a separate TLS 1.3 raw-public-key endpoint. Discovery is only a
connection hint. Both users must confirm the full key fingerprints before trust
is saved. The regressions cover refusal before confirmation, reflection and
fingerprint mismatch, cancellation, bounded frames, fragmented concurrent I/O,
expiry and acknowledgment of durable authorization.

## Release gates still open

- Developer ID signing, notarization, stapling and Gatekeeper acceptance of the
  actual macOS release artifact.
- Packaged installation and pairing on a physical Mac and supported Linux desktop.
- Login/logout, sleep/wake and network interruption with independent clipboard
  and service observations on those machines.
- Linux distribution/desktop compatibility beyond the disposable X11/Sway image.

Unsigned development artifacts and fixture package tests do not satisfy those
gates. The festival's implementation report records the exact commands, results
and remaining acceptance work for this change.

## Implementation candidate verification (2026-10-19)

- Rust 1.95: workspace/all-features Clippy with warnings denied, formatting and
  doc tests passed. macOS: 323 tests passed, 2 ignored; disposable Linux: 321
  passed, 3 ignored, followed by a separate passing real-multicast mDNS test.
- Native macOS: GUI, PTY, Unicode history, two-window mutual enrollment and
  revocation, menu-bar close/reopen/pause/quit, and first-run setup passed.
- Disposable Linux: real X11 and headless Sway GUI, PTY and full Unicode Copy
  passed. No physical Linux desktop or login/sleep claim follows from this.
- Packaging fixtures: 82 passed on macOS and 77 on Linux. Existing Linux
  installer harness: 11 passed. Release macOS bundle launched with the packaged
  CLI and no test inspection listener; static bundle and DMG verification passed.

The default ignored cases are an existing mock-transport test and the multicast
test; Linux additionally skips its X11-only unit test outside a display session.
Multicast was explicitly tested separately in Linux; real X11 clipboard behavior
was exercised by the GUI/PTY harness. The pre-existing mock test remains skipped.
The packaging fixtures simulate signing failure paths; no Developer ID signature
or Apple notarization was obtained. Local evidence lives under
`target/companion-evidence/` and `target/companion-linux-evidence/` and is excluded
from version control. The README screenshot contains isolated fixture data.
