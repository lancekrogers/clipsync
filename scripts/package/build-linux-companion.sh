#!/usr/bin/env bash
# Assemble Linux user-local companion archive (GUI + CLI/TUI + desktop entry + icon).
# No root/system packages, Flatpak, or AppImage. Does not mutate the host service.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

ROOT="$(clipsync_package_project_root)"
VERSION="$(clipsync_package_version)"
ARCH="$(clipsync_package_arch)"
DIST="$(clipsync_package_dist_root)/linux"
TARGET_DIR="$(clipsync_package_target_dir)"
LABEL="$(clipsync_package_artifact_label_unsigned)"
SKIP_BUILD="${CLIPSYNC_PACKAGE_SKIP_BUILD:-0}"
HOST_OS="$(uname -s)"

# Host guard: never package Darwin Mach-O (or other) as a Linux archive unless
# an explicit fixture/test mode is opted in with override binaries.
if [[ "$HOST_OS" != "Linux" ]]; then
  if clipsync_package_fixture_mode; then
    clipsync_package_warn "build-linux on $HOST_OS under FIXTURE mode — outputs are test fixtures, not a Linux release archive"
  else
    clipsync_package_die "build-linux-companion.sh requires a Linux host (got $HOST_OS). For script tests set CLIPSYNC_PACKAGE_FIXTURE_MODE=1 and CLIPSYNC_PACKAGE_SKIP_BUILD=1 with override bins."
  fi
fi

BUNDLE_NAME="clipsync-companion-${VERSION}-${ARCH}"
BUNDLE_DIR="$DIST/$BUNDLE_NAME"
ARCHIVE_NAME="${BUNDLE_NAME}-${LABEL}.tar.gz"
ARCHIVE_PATH="$DIST/$ARCHIVE_NAME"

mkdir -p "$DIST"

if [[ "$SKIP_BUILD" != "1" ]]; then
  bash "$SCRIPT_DIR/build-binaries.sh" >/dev/null
fi

CLI_BIN="${CLIPSYNC_PACKAGE_CLI_BIN:-$TARGET_DIR/release/clipsync}"
GUI_BIN="${CLIPSYNC_PACKAGE_GUI_BIN:-$TARGET_DIR/release/clipsync-desktop}"
[[ -f "$CLI_BIN" ]] || clipsync_package_die "CLI binary missing: $CLI_BIN"
[[ -f "$GUI_BIN" ]] || clipsync_package_die "GUI binary missing: $GUI_BIN"

# Format / arch enforcement (fixture scripts allowed only in fixture mode).
clipsync_package_require_binary_format "$CLI_BIN" linux "$GUI_BIN" >/dev/null
clipsync_package_require_binary_format "$GUI_BIN" linux "$CLI_BIN" >/dev/null

SVG_SRC="$ROOT/assets/clipsync.svg"
[[ -f "$SVG_SRC" ]] || clipsync_package_die "missing icon: $SVG_SRC"

clipsync_package_refuse_overwrite "$BUNDLE_DIR"
clipsync_package_refuse_overwrite "$ARCHIVE_PATH"

clipsync_package_log "assembling $BUNDLE_DIR"

mkdir -p \
  "$BUNDLE_DIR/bin" \
  "$BUNDLE_DIR/share/applications" \
  "$BUNDLE_DIR/share/icons/hicolor/scalable/apps" \
  "$BUNDLE_DIR/share/doc/clipsync" \
  "$BUNDLE_DIR/lib"

cp "$CLI_BIN" "$BUNDLE_DIR/bin/clipsync"
cp "$GUI_BIN" "$BUNDLE_DIR/bin/clipsync-desktop"
chmod 755 "$BUNDLE_DIR/bin/clipsync" "$BUNDLE_DIR/bin/clipsync-desktop"
cp "$SVG_SRC" "$BUNDLE_DIR/share/icons/hicolor/scalable/apps/clipsync.svg"

# Template desktop entry: Exec placeholder replaced by install-user.sh with a
# properly escaped absolute path (two-layer Desktop Entry + Exec escaping).
# Relative Exec=./bin/... is intentionally NOT shipped — file managers resolve
# relative Exec against the launcher CWD, not the .desktop location.
cat >"$BUNDLE_DIR/share/applications/clipsync.desktop.in" <<'EOF'
[Desktop Entry]
Type=Application
Name=ClipSync
Comment=Clipboard sync companion (GUI)
Exec=@CLIPSYNC_DESKTOP_EXEC@
Icon=clipsync
Terminal=false
Categories=Utility;Network;
StartupNotify=true
EOF

# Preserve existing installer helper scripts as optional references (not invoked here).
if [[ -f "$ROOT/scripts/lib/linux_user_service.sh" ]]; then
  cp "$ROOT/scripts/lib/linux_user_service.sh" "$BUNDLE_DIR/lib/"
fi

clipsync_package_write_provenance "$BUNDLE_DIR/share/doc/clipsync/PROVENANCE.json" "linux-user-archive"

cat >"$BUNDLE_DIR/share/doc/clipsync/UNINSTALL.md" <<'EOF'
# Uninstall ClipSync companion (user-local)

## Remove managed application resources

```bash
# Binaries (and retained previous copies from upgrades)
rm -f ~/.local/bin/clipsync ~/.local/bin/clipsync.previous
rm -f ~/.local/bin/clipsync-desktop ~/.local/bin/clipsync-desktop.previous

# Desktop launcher + icon
rm -f ~/.local/share/applications/clipsync.desktop
rm -f ~/.local/share/icons/hicolor/scalable/apps/clipsync.svg
update-desktop-database ~/.local/share/applications 2>/dev/null || true
```

If a user systemd unit was installed by Companion setup (not by this archive alone):

```bash
systemctl --user stop clipsync.service 2>/dev/null || true
systemctl --user disable clipsync.service 2>/dev/null || true
rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/clipsync.service"
systemctl --user daemon-reload 2>/dev/null || true
```

On macOS, remove `ClipSync.app` and optionally `~/Library/LaunchAgents/com.clipsync.plist`
(only if Companion setup created it).

## Preserved by default (do NOT delete unless you intend to)

Uninstall of binaries/service resources preserves:

- Config (`~/.config/clipsync/` or macOS Application Support equivalent)
- Dedicated identity keys referenced by config
- Authorized peers / trust cache
- Encrypted history database and history key

Data deletion is a separate explicit action. This package does not invent migration
or wipe routines.
EOF

cat >"$BUNDLE_DIR/share/doc/clipsync/LIMITATIONS.md" <<'EOF'
# Package installer limitations

- No automatic service health check after binary swap. If an upgrade leaves a
  broken daemon, restore from `*.previous` manually; the installer never restarts
  systemd/launchd.
- Terminal-free install entrypoint is `ClipSync-Install.desktop` (gio / file
  manager "Activate"). Double-clicking `install-user.sh` directly is **not**
  a supported entrypoint — desktop environments differ and may open an editor.
- `ClipSync-Install.desktop` defaults to Freedesktop `%k` so a file manager
  Activate finds `install-user.sh` beside the `.desktop` after extract anywhere.
- Stock `gio launch` on glib 2.74 does not expand `%k` / set
  `GIO_LAUNCHED_DESKTOP_FILE`; use file-manager Activate, `./install-user.sh`,
  or `./write-install-launcher.sh --absolute` then `gio launch`.
- Relative `Exec=./…` is not shipped (CWD-relative; breaks under file managers).
EOF

FIXTURE_NOTE=""
if clipsync_package_fixture_mode; then
  FIXTURE_NOTE="FIXTURE MODE: binaries are overrides/scripts; not a verified Linux ELF release."
fi

VERSION_SAFE="$(clipsync_json_escape "$VERSION")"
ARCH_SAFE="$(clipsync_json_escape "$ARCH")"
LABEL_SAFE="$(clipsync_json_escape "$LABEL")"

cat >"$BUNDLE_DIR/share/doc/clipsync/README-${LABEL}.txt" <<EOF
ClipSync companion ${VERSION} (${ARCH}) — ${LABEL}
${FIXTURE_NOTE}

Not a distro package and not a public signed release.
Runtime libraries (e.g. libxkbcommon-x11 on X11) must be provided by the system;
see docs/COMPANION.md.

Layout:
  bin/clipsync            CLI / daemon / TUI (feature tui when built from cargo)
  bin/clipsync-desktop    GUI (sibling of CLI)
  share/applications/     desktop entry template (absolute Exec written at install)
  share/icons/.../clipsync.svg

Terminal-free install (supported):
  From the extracted folder (any path), Activate ClipSync-Install.desktop in a
  graphical file manager. Exec uses %k (Terminal=false, no root).

Shell: ./install-user.sh
gio launch CLI: ./write-install-launcher.sh --absolute && gio launch ClipSync-Install.desktop

Not supported as an installer UX claim:
  Double-clicking install-user.sh as a raw shell script.
EOF

# Minimal scoped installer: user-writable prefixes only, no privilege escalation.
# Uses CLIPSYNC_INSTALL_PREFIX / XDG_* ; does not require mutating the real host HOME
# when those are set (tests should use a fixture prefix env).
cat >"$BUNDLE_DIR/install-user.sh" <<'INSTALL'
#!/usr/bin/env bash
# User-local companion installer (no sudo / no system unit writes).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

desktop_escape_exec_arg() {
  local value="$1" exec_level="" out="" i c
  local -i n
  if [[ "$value" == *$'\n'* || "$value" == *$'\r'* ]]; then
    echo "desktop Exec path must not contain newlines" >&2
    return 1
  fi
  if [[ "$value" == *'='* ]]; then
    echo "desktop Exec executable path must not contain '='" >&2
    return 1
  fi
  n=${#value}
  for ((i = 0; i < n; i++)); do
    c="${value:i:1}"
    case "$c" in
      \\) exec_level+='\\' ;;
      \") exec_level+='\"' ;;
      \$) exec_level+='\$' ;;
      \`) exec_level+='\`' ;;
      %) exec_level+='%%' ;;
      *) exec_level+="$c" ;;
    esac
  done
  exec_level="\"${exec_level}\""
  n=${#exec_level}
  for ((i = 0; i < n; i++)); do
    c="${exec_level:i:1}"
    case "$c" in
      \\) out+='\\' ;;
      *) out+="$c" ;;
    esac
  done
  printf '%s' "$out"
}

# Pair install with rollback of first binary if second fails. No service restart.
install_binary_pair_safe() {
  local src_cli="$1" dst_cli="$2" src_gui="$3" dst_gui="$4"
  local cli_had=0 tmp
  [[ -f "$src_cli" ]] || { echo "missing: $src_cli" >&2; return 1; }
  [[ -f "$src_gui" ]] || { echo "missing: $src_gui" >&2; return 1; }
  mkdir -p "$(dirname "$dst_cli")" "$(dirname "$dst_gui")"

  if [[ -e "$dst_cli" ]]; then
    rm -f "${dst_cli}.previous"
    cp "$dst_cli" "${dst_cli}.previous"
    chmod 755 "${dst_cli}.previous" 2>/dev/null || true
    cli_had=1
  fi
  if [[ -e "$dst_gui" ]]; then
    rm -f "${dst_gui}.previous"
    cp "$dst_gui" "${dst_gui}.previous"
    chmod 755 "${dst_gui}.previous" 2>/dev/null || true
  fi

  tmp=$(mktemp "${dst_cli}.XXXXXX") || return 1
  if ! cp "$src_cli" "$tmp" || ! chmod 755 "$tmp" || ! mv -f "$tmp" "$dst_cli"; then
    rm -f "$tmp"
    return 1
  fi

  if [[ "${CLIPSYNC_PACKAGE_INJECT_PAIR_FAIL:-}" == "gui" ]]; then
    if [[ "$cli_had" -eq 1 ]]; then
      cp "${dst_cli}.previous" "$dst_cli"
      chmod 755 "$dst_cli"
    else
      rm -f "$dst_cli"
    fi
    echo "GUI install failed (injected); CLI rolled back (no service restart)." >&2
    return 1
  fi

  tmp=$(mktemp "${dst_gui}.XXXXXX") || true
  if [[ -z "${tmp:-}" ]] || ! cp "$src_gui" "$tmp" || ! chmod 755 "$tmp" || ! mv -f "$tmp" "$dst_gui"; then
    rm -f "${tmp:-}"
    if [[ "$cli_had" -eq 1 ]]; then
      cp "${dst_cli}.previous" "$dst_cli"
      chmod 755 "$dst_cli"
    else
      rm -f "$dst_cli"
    fi
    echo "GUI install failed; CLI rolled back (no service restart). Repair via *.previous if needed." >&2
    return 1
  fi
  return 0
}

# Fixture / test prefix: prefer explicit install prefix over real HOME.
PREFIX="${CLIPSYNC_INSTALL_PREFIX:-${CLIPSYNC_PACKAGE_FIXTURE_PREFIX:+${CLIPSYNC_PACKAGE_FIXTURE_PREFIX}/.local}}"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="${CLIPSYNC_INSTALL_DIR:-$PREFIX/bin}"
DATA_HOME="${XDG_DATA_HOME:-${CLIPSYNC_PACKAGE_FIXTURE_PREFIX:+${CLIPSYNC_PACKAGE_FIXTURE_PREFIX}/.local/share}}"
DATA_HOME="${DATA_HOME:-$HOME/.local/share}"
APP_DIR="$DATA_HOME/applications"
ICON_DIR="$DATA_HOME/icons/hicolor/scalable/apps"

echo "ClipSync companion — user-local install"
echo "  prefix binaries: $BIN_DIR"
echo "  desktop entry:   $APP_DIR/clipsync.desktop"
echo "  (no root; no service enable/restart; Companion GUI owns service setup)"

install_binary_pair_safe \
  "$SCRIPT_DIR/bin/clipsync" "$BIN_DIR/clipsync" \
  "$SCRIPT_DIR/bin/clipsync-desktop" "$BIN_DIR/clipsync-desktop"

mkdir -p "$APP_DIR" "$ICON_DIR"
install -m 644 "$SCRIPT_DIR/share/icons/hicolor/scalable/apps/clipsync.svg" "$ICON_DIR/clipsync.svg"

DESKTOP_BIN="$BIN_DIR/clipsync-desktop"
# Prefer realpath/absolute so file-manager Exec never depends on CWD.
if command -v realpath >/dev/null 2>&1; then
  DESKTOP_BIN="$(realpath "$DESKTOP_BIN")"
else
  DESKTOP_BIN="$(cd "$(dirname "$DESKTOP_BIN")" && pwd)/$(basename "$DESKTOP_BIN")"
fi
ESCAPED="$(desktop_escape_exec_arg "$DESKTOP_BIN")"
sed "s|@CLIPSYNC_DESKTOP_EXEC@|${ESCAPED}|" \
  "$SCRIPT_DIR/share/applications/clipsync.desktop.in" \
  >"$APP_DIR/clipsync.desktop"
chmod 644 "$APP_DIR/clipsync.desktop"

if grep -E 'Exec=.*\$\(|Exec=.*`' "$APP_DIR/clipsync.desktop" >/dev/null; then
  echo "refusing unsafe Exec= in desktop file" >&2
  exit 1
fi

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIR" 2>/dev/null || true
fi

# Optional desktop notification for Terminal=false install feedback.
if command -v notify-send >/dev/null 2>&1; then
  notify-send "ClipSync" "User-local install complete" 2>/dev/null || true
fi

STATUS_FILE="${CLIPSYNC_PACKAGE_FIXTURE_PREFIX:-$PREFIX}/clipsync-install-status.txt"
mkdir -p "$(dirname "$STATUS_FILE")" 2>/dev/null || true
printf 'ok installed_gui=%s\n' "$DESKTOP_BIN" >"$STATUS_FILE" 2>/dev/null || true

echo "Installed."
echo "Launch: $DESKTOP_BIN   or your app menu entry 'ClipSync'."
echo "TUI:    $BIN_DIR/clipsync tui"
echo "Uninstall docs: $SCRIPT_DIR/share/doc/clipsync/UNINSTALL.md"
echo "Config/identity/trust/history are not modified by this installer."
echo "No service health check or restart was performed."
INSTALL
chmod 755 "$BUNDLE_DIR/install-user.sh"

# Terminal-free file-manager installer (D05): Terminal=false + portable Exec.
#
# Critical: do NOT bake absolute paths to install-user.sh at package time — those
# break when the archive is extracted elsewhere. Relative Exec=./… is also wrong:
# launchers resolve relative Exec against CWD (or Path=), not the .desktop location.
#
# Portable default: Freedesktop %k + parameter expansion only (no $(…) / backticks).
# File managers use GLib DesktopAppInfo, which expands %k and/or sets
# GIO_LAUNCHED_DESKTOP_FILE. Execute install-user.sh by path so its bash shebang
# is honored (never `sh install-user.sh` — dash rejects `pipefail`).
#
# Note (glib 2.74 / gio CLI): `gio launch` does NOT expand %k and does NOT set
# GIO_LAUNCHED_DESKTOP_FILE. For that CLI, re-run this script with --absolute
# after extract, or invoke ./install-user.sh from a shell.
cat >"$BUNDLE_DIR/write-install-launcher.sh" <<'LAUNCH'
#!/usr/bin/env bash
# Write ClipSync-Install.desktop (portable %k by default, or --absolute for gio CLI).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$SCRIPT_DIR/ClipSync-Install.desktop"
MODE="portable"
case "${1:-}" in
  --absolute) MODE="absolute" ;;
  --portable|"") MODE="portable" ;;
  -h|--help)
    echo "Usage: $0 [--portable|--absolute]"
    exit 0
    ;;
  *)
    echo "unknown option: $1 (use --portable or --absolute)" >&2
    exit 2
    ;;
esac

escape() {
  local value="$1" exec_level="" out="" i c
  local -i n
  if [[ "$value" == *$'\n'* || "$value" == *$'\r'* || "$value" == *'='* ]]; then
    echo "refusing unsafe path for desktop Exec: $value" >&2
    return 1
  fi
  n=${#value}
  for ((i = 0; i < n; i++)); do
    c="${value:i:1}"
    case "$c" in
      \\) exec_level+='\\' ;;
      \") exec_level+='\"' ;;
      \$) exec_level+='\$' ;;
      \`) exec_level+='\`' ;;
      %) exec_level+='%%' ;;
      *) exec_level+="$c" ;;
    esac
  done
  exec_level="\"${exec_level}\""
  n=${#exec_level}
  for ((i = 0; i < n; i++)); do
    c="${exec_level:i:1}"
    case "$c" in
      \\) out+='\\' ;;
      *) out+="$c" ;;
    esac
  done
  printf '%s' "$out"
}

if [[ "$MODE" == "absolute" ]]; then
  INSTALL_SCRIPT="$SCRIPT_DIR/install-user.sh"
  if command -v realpath >/dev/null 2>&1; then
    INSTALL_SCRIPT="$(realpath "$INSTALL_SCRIPT")"
  fi
  SCRIPT_ESCAPED="$(escape "$INSTALL_SCRIPT")"
  cat >"$OUT" <<EOF
[Desktop Entry]
Type=Application
Name=Install ClipSync (user-local)
Comment=Copy ClipSync companion into the user prefix — no administrator privileges
Exec=${SCRIPT_ESCAPED}
Terminal=false
Categories=Utility;
StartupNotify=false
EOF
  echo "Wrote $OUT (absolute Exec for this extract dir; needed for gio launch CLI)"
else
  # Literal '%' in Exec must be '%%' (so ${f%%/*} → ${f%/*} after field-code decode).
  # Prefer %k; also accept GIO_LAUNCHED_DESKTOP_FILE when the DE sets it.
  cat >"$OUT" <<'EOF'
[Desktop Entry]
Type=Application
Name=Install ClipSync (user-local)
Comment=Copy ClipSync companion into the user prefix — no administrator privileges
Exec=/bin/sh -c "f=\${1:-}; if [ -z \"\$f\" ]; then f=\$GIO_LAUNCHED_DESKTOP_FILE; fi; f=\${f#file://}; d=\${f%%/*}; exec \"\$d/install-user.sh\"" sh %k
Terminal=false
Categories=Utility;
StartupNotify=false
EOF
  echo "Wrote $OUT (portable %k for file-manager Activate)"
fi
chmod 644 "$OUT"
echo "Terminal-free install: activate this .desktop in a file manager (Nautilus/Dolphin/etc)."
echo "Shell install: \"$SCRIPT_DIR/install-user.sh\""
echo "gio launch CLI (glib without %k): \"$SCRIPT_DIR/write-install-launcher.sh\" --absolute && gio launch \"$OUT\""
echo "Not supported: double-clicking install-user.sh as a raw shell script."
LAUNCH
chmod 755 "$BUNDLE_DIR/write-install-launcher.sh"
( cd "$BUNDLE_DIR" && ./write-install-launcher.sh --portable >/dev/null )

# Archive
(
  cd "$DIST"
  tar -czf "$ARCHIVE_NAME" "$BUNDLE_NAME"
)

MANIFEST="$DIST/MANIFEST-${VERSION}-${ARCH}.json"
FIXTURE_JSON=false
FEATURES_VERIFIED=true
if clipsync_package_fixture_mode; then
  FIXTURE_JSON=true
  FEATURES_VERIFIED=false
fi
cat >"$MANIFEST" <<EOF
{
  "version": "$VERSION_SAFE",
  "arch": "$ARCH_SAFE",
  "signing": "$LABEL_SAFE",
  "public_release_ready": false,
  "fixture_mode": $FIXTURE_JSON,
  "features_verified": $FEATURES_VERIFIED,
  "bundle": "$(clipsync_json_escape "$BUNDLE_NAME")",
  "archive": "$(clipsync_json_escape "$ARCHIVE_NAME")",
  "linux_layout": {
    "cli": "bin/clipsync",
    "gui": "bin/clipsync-desktop",
    "desktop_template": "share/applications/clipsync.desktop.in",
    "icon": "share/icons/hicolor/scalable/apps/clipsync.svg",
    "installer": "install-user.sh",
    "terminal_free_entrypoint": "ClipSync-Install.desktop"
  },
  "notes": "ClipSync-Install.desktop uses %k (not relative Exec or package-time absolute paths). Raw .sh double-click is not a supported install UX."
}
EOF

clipsync_package_log "Linux package complete (unsigned development archive)"
clipsync_package_log "  bundle: $BUNDLE_DIR"
clipsync_package_log "  archive: $ARCHIVE_PATH"
clipsync_package_log "  manifest: $MANIFEST"
if clipsync_package_fixture_mode; then
  clipsync_package_log "  FIXTURE MODE — not a verified host-arch Linux release"
fi
