# ClipSync companion (desktop GUI + TUI)

User-facing guide for the **packaged** companion. This is not a public notarized
release guide: local development packages are labeled **UNSIGNED-DEVELOPMENT**.

## What you get

| Piece | Role |
|---|---|
| `clipsync-desktop` | Native GUI (eframe). Terminal-free setup via shared Companion. |
| `clipsync` | Matching CLI / user daemon / `clipsync tui` (built with `--features tui`) |
| Desktop entry + SVG (Linux) | App-menu launch to the **absolute** user-installed GUI path |
| ClipSync.app (macOS) | `Contents/MacOS/clipsync-desktop` + sibling `clipsync` (+ `AppIcon.icns` when converters are available) |

GUI and TUI share the same config, identity, trust, and history. There is one
user-owned daemon; closing the app does not stop the daemon.

## Install from a local package

### macOS (UNSIGNED development DMG / .app)

1. Open the DMG named `*-UNSIGNED-DEVELOPMENT.dmg`.
2. Drag `ClipSync.app` to `/Applications` or `~/Applications`.
3. First launch of an unsigned app may be blocked by Gatekeeper — expected for
   this artifact. Do **not** treat Gatekeeper bypass as the intended public UX.
4. Open ClipSync and complete on-screen setup (no terminal required).

The packaged GUI finds the daemon at `ClipSync.app/Contents/MacOS/clipsync`
(sibling of the GUI binary).

### Linux (user-local archive)

1. Extract `clipsync-companion-*-UNSIGNED-DEVELOPMENT.tar.gz` to any directory
   (Downloads, a USB path, etc.).
2. **Terminal-free install (supported):** Activate `ClipSync-Install.desktop` in a
   graphical file manager (Nautilus, Dolphin, Thunar, …). The entry uses
   Freedesktop `%k` so GLib DesktopAppInfo locates sibling `install-user.sh`
   beside the `.desktop` — independent of your shell CWD, and without a
   package-build absolute path (those break after extract/move).
   Runs with `Terminal=false` (no administrator privileges; no system service).
3. **Shell install (supported):** `./install-user.sh` from the extract dir.
4. **`gio launch` CLI note:** on glib 2.74, `gio launch` does **not** expand `%k`
   and does not set `GIO_LAUNCHED_DESKTOP_FILE`. For that CLI only, either run
   `./install-user.sh`, or:
   `./write-install-launcher.sh --absolute && gio launch ClipSync-Install.desktop`
   (`--absolute` bakes Exec for the **current** extract directory; re-run after move).
5. **Not supported as an installer UX:** double-clicking `install-user.sh` as a
   raw shell script (desktop environments may open an editor instead).
6. Launch **ClipSync** from the app menu, or run `~/.local/bin/clipsync-desktop`.
7. Complete setup in the GUI. Service enablement uses the shared Companion/service
   helpers (user systemd unit only — never a root unit from this package).

After install, `install-user.sh` writes the app-menu `.desktop` `Exec=` with a
**two-layer-escaped absolute path** to the installed GUI (Desktop Entry string
escaping + Exec quoting, including `%%` for literal `%`). That installed launcher
is CWD-independent. The archive does **not** ship relative `Exec=./bin/...`
launchers (those resolve against the launcher CWD and break under file managers).

TUI (same daemon/config):

```bash
~/.local/bin/clipsync tui
```

## Pair two computers

1. On one computer, open **Devices** and choose **Allow a new device** (TUI:
   `2`, then `e`). A six-digit code appears with a countdown.
2. On the other computer, open **Devices** and pick the first computer under
   **Nearby** (TUI: `f`, select it, `Enter`), or enter the address shown under
   the code (TUI: `m`). Then type the code.
3. Both computers save the pairing and list each other under **Authorized
   devices** with the device name and key fingerprint.

Each code works once. After a try, or after two minutes, a new code appears.
One window accepts five tries and closes after ten minutes or once a device has
paired. Neither computer saves anything until the code is verified, so a
mistyped code never pairs anything. While a code is being typed in
the TUI, digits stay in the field and `q` does not quit; `Esc` cancels.

When pairing fails, the computer that typed the code says why:

| Message | What to do |
|---|---|
| Nothing is accepting pairing at that address | Choose **Allow a new device** on the other computer. If it is already showing a code, a firewall may be blocking the pairing port (the sync port + 1, 8485 by default); ClipSync does not change firewall rules. |
| The other computer has no pairing code open | Its window used its last code. Choose **Allow a new device** there again. |
| That code did not match | The code was wrong, expired or already used. Type the new code now on the other screen. |
| The other computer runs an older ClipSync | Update ClipSync there. Older versions compared fingerprints and cannot pair by code. |
| Saved on this computer, but the other computer did not confirm | Remove the device under Authorized devices and pair again. |

Both computers need this version. A companion or service without code pairing
shows an update message instead of the pairing controls. Devices paired
before this change keep syncing; their authorization is unchanged.

**Find nearby** listens for mDNS announcements on the local network for two
minutes; a computer is announced only while it is allowing a device.

## Upgrade

- Packaging **refuses to overwrite** an existing output directory/archive unless
  `CLIPSYNC_PACKAGE_FORCE=1` (and still refuses dangerous paths such as `/`).
- The Linux installer replaces **CLI and GUI as a pair** via temp+rename. Both
  previous binaries are retained as `*.previous` before either swap. If the GUI
  swap fails, the CLI is rolled back so the installed pair is not left mismatched.
- The package installer does **not** restart or health-check the user service.
  Manual repair: restore from `*.previous` if needed.
- No automatic data migration is performed. Config, identity, authorized peers,
  trust cache, history DB and history key are left untouched across upgrades.

## Uninstall (preserves data)

See `share/doc/clipsync/UNINSTALL.md` inside the Linux archive. Summary:

**Remove** managed binaries, desktop entry, icon, and (if present) the user
service unit / LaunchAgent that Companion setup created.

**Preserve by default:**

- Config under `~/.config/clipsync/` (and any macOS Application Support path in use)
- Dedicated identity keys referenced by config
- Authorized peers / trust cache
- Encrypted history database and history key

Deleting that state is a separate explicit action.

## Building packages (developers)

```bash
# Optional: pin GUI toolchain without changing rustup default
export CLIPSYNC_RUSTUP_TOOLCHAIN=1.95.0

./scripts/package_companion.sh all
# or: just companion-package
```

Builds:

- `clipsync` with `--locked --features tui` only (no `integration-tests`)
- `clipsync-desktop` with `--locked` without `ui-test`

Fixture / script tests (no host service changes; not release proof):

```bash
export CLIPSYNC_PACKAGE_FIXTURE_MODE=1 CLIPSYNC_PACKAGE_SKIP_BUILD=1
./scripts/package_companion.sh test
./scripts/package_companion.sh verify --linux dist/companion/linux/clipsync-companion-…
```

`build-linux-companion.sh` refuses to run on a non-Linux host unless fixture mode is set,
so Darwin Mach-O binaries cannot be labeled as a Linux archive by accident.
Fixture provenance sets `fixture_mode: true` and `features.verified: false`.

### Signing / notarization (optional, gated)

`just companion-sign-macos` / `./scripts/package_companion.sh sign-macos` requires
explicit `CLIPSYNC_CODESIGN_IDENTITY` and notary credentials. This environment
has **no** Developer ID Application identity for distribution.

Correct release flow (implemented by `sign-macos.sh`):

1. Codesign nested CLI/GUI, then the `.app`
2. Zip the `.app` (`ditto`) — notarytool cannot submit a bare `.app` folder
3. `notarytool submit` the zip; `stapler staple` the `.app`
4. **Recreate** the DMG from the signed+stapled `.app` (an older DMG still
   contains the unsigned bundle; signing the DMG container alone is not enough)
5. Codesign the new DMG (failure is fatal), notarize + staple the DMG
6. Write `ACCEPTANCE.json`; only `status=accepted` may set `public_release_ready`

Unsigned local artifacts must not be labeled notarized or public-release-ready (D05).

## Limitations

- Apple Silicon (or the host arch you built) only — do not advertise Intel or an
  older macOS minimum from a successful arm64 build.
- Linux clipboard still depends on compositor support (e.g. wl-clipboard /
  data-control). X11 needs runtime libs such as `libxkbcommon-x11`.
- Distro repos, Flatpak, AppImage, and auto-update are deferred.
- Default packaging never creates root/system services or modifies firewalls.
- Existing `scripts/install_user.sh` / daemon-only installers remain for CLI-only
  flows; companion packages are additive under `dist/companion/`.
- Package installer performs no service health check after upgrade.

## Artifact labeling

| Label | Meaning |
|---|---|
| `UNSIGNED-DEVELOPMENT` | Local/dev build; not Developer ID signed; not notarized |
| `public_release_ready: false` | Must stay false until signed+notarized acceptance exists |
| `fixture_mode: true` | Override/script binaries used for packaging tests — not release proof |

## Verification

See [companion acceptance](COMPANION_ACCEPTANCE.md) for reproducible native, terminal,
Linux and packaging checks and the remaining release gates.
