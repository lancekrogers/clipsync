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
| Reconciliation | Restart identity and generation ordering, disconnected action gating, rapid selection changes, old-daemon capability mismatch; real GUI/TUI malformed/disconnected IPC and recovery |
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

## Local-control recovery harness

`companion_recovery.py` runs as part of both native and disposable Linux runners.
It interposes a temporary same-user socket proxy on the isolated fixture, first
closes requests and then returns malformed JSON. The actual GUI and TUI must both
show Service unavailable, reject pause actions during the outage, recover without
relaunch, and converge when pause is changed through one interface and resumed
through the other. The PTY must restore terminal modes and cursor on exit.
The original fixture socket is restored in cleanup. This verifies IPC recovery;
it does not substitute for a physical reboot or remote-network lifecycle test.

On the root-owned disposable X11 fixture, `companion_control_uid.py` temporarily
allows traversal to the test socket and connects as `nobody`. The daemon must
reject that connection without responding, then still answer its own user.
The harness restores the fixture permissions. This tests kernel peer credentials
independently of the directory's usual protection.

## Local-control follow-up (2026-10-20)

- Rust 1.95 workspace/all-features tests: 350 passed, 2 ignored on macOS;
  348 passed, 3 ignored in Debian arm64. Formatting and workspace/all-features
  Clippy with warnings denied passed on macOS. The ignored cases remain those
  described above; multicast was not rerun for this control-only change.
- Native macOS GUI/PTY, malformed/disconnected IPC recovery, full Unicode history,
  mutual enrollment/revocation and isolated first-run setup passed.
- Disposable X11 and Sway GUI/PTY, IPC recovery and full Unicode history passed.
  X11 also passed the actual wrong-user credential check.
- The macOS tray rerun could not click the icon because Hidden Bar positioned it
  outside every display. Native close-to-tray succeeded, but tray actions are
  **unverified on this revision**. The earlier candidate's tray result above is
  historical evidence, not a pass for this rerun.

Control calls stop pending handler work on disconnect or the three-second server
deadline. Completed writes and bytes already handed to a peer cannot be recalled.
Read retries retain the latest request separately for each view; mutations are
not retried automatically. Incompatible protocol replies direct users to update
instead of restart.

Diagnostics include at most 20 recent failures from the current control server,
using fixed allowlisted codes/messages and timestamps. They reset on daemon
restart and do not collect host logs. Tests check sentinel clipboard text, keys,
peer labels and paths are absent from exports, rendered errors and control debug
logs. No history format, identity, service installation or dependency changes
are part of this follow-up.

## Pairing follow-up (2026-10-20)

New dial results identify the session they created, so retrying after a failure
selects that attempt in the TUI. The optional local-control field
`EnrollmentSnapshot.requested_session_id` is absent on ordinary polls; older
snapshots remain readable. Enrollment wire messages and trust formats are unchanged.

Both connection directions now check the TLS fingerprint's failure budget before
processing another verification attempt. Regressions cover repeated mismatches,
independent rate-limit expiry, confirm replay, full session capacity, wrong ALPN,
reverse dialing and identical display names with distinct authenticated keys.
The trust harness enrolls two peers, opens production sync with the saved pins,
revokes during a live connection, rejects reconnect and reloads the trust store.
A separate write-failure case preserves prior pins, avoids false success and
successfully retries after the fixture fault is removed.

`companion_enrollment_ui.py` now runs on Linux as well as macOS and checks the
visible waiting state after one confirmation. `companion_enrollment_recovery.py`
drives unreachable addresses and cancellation through both the real GUI and PTY,
compares full fingerprints, checks both trust files remain byte-identical, and
verifies terminal restoration. All network targets and clipboard data are isolated.
Physical two-device and packaged lifecycle acceptance remain separate gates.

## Desktop activation acceptance

Opening the same configuration again asks the running desktop to show its window
and exits without constructing another companion. Distinct explicit config paths
remain independent. Existing symlink aliases resolve to the same instance; default
first-run setup retains its identity when its config is created. The owner uses a
private per-user socket with UID checks and a kernel-held lock. A crash releases
the lock; only its successor may replace the stale, same-user socket.

`just companion-test-instance-macos` runs the isolated duplicate-launch suite.
The regular Mac and Linux acceptance runners also include it. The suite verifies
duplicate and alias launches, route preservation, independent config windows,
app-crash recovery, and an unchanged daemon PID. On macOS it presses the test
window's native close button and checks that relaunch reveals that same process.
The harness uses synthetic history, private pasteboards, and disposable Linux
desktops. Process cleanup is not counted as a native Quit-menu test; compositor
focus and packaged Launch Services reopening have separate acceptance limits.

## Real daemon crash/restart acceptance

Run `just companion-test-restart` on macOS, or in a disposable Linux desktop
with `CLIPSYNC_DISPOSABLE_DESKTOP=1`. It requires `uv` and Rust 1.95 and honors
`CARGO_TARGET_DIR`. Do not run the Linux fixture in a personal desktop session.
The Mac runner generates its own private named pasteboard.

Unlike the socket-proxy recovery test, this runner kills the actual fixture
process and starts a new daemon on the same configuration and socket. The live
TUI must show Service unavailable, recover without relaunch, and successfully
pause the new daemon. It verifies new PID/instance identity, unchanged device
identity and history key, readable unchanged encrypted history, reset pause
state, and terminal restoration while the daemon stays running after TUI exit.
Results and terminal screens are saved under `target/companion-evidence/restart/`.

The integration-only `companion_fixture --resume DIRECTORY` accepts a retained
fixture directory, reconstructs paths within it, and rejects unmarked directories,
symlinked state files and a live daemon's lock before rewriting state. Fresh
fixtures clean up on normal exit. SIGKILL leaves their directory behind;
callers resuming a fixture own its final cleanup. The regression runner performs
that cleanup and checks all three rejection paths.

On 2026-10-23 this regression passed on macOS and disposable Debian ARM64/X11.
Manual native GUI interaction also verified unavailable/recovered states on both,
Mac close/hide and same-instance reopening, Mac native Quit leaving the daemon
running, and Linux no-tray close/reopen. Mac Tab/Shift-Tab focus and text zoom
passed. Linux history search and keyboard result selection passed. Linux zoom,
current native tray actions and physical login/sleep acceptance remain unverified;
these fixture results do not close those gates. No production code changed.
