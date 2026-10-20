#!/usr/bin/env bash
# Isolated packaging tests — temp dirs / fixture prefix only; no host service changes.
# Does not mutate the real user HOME for install targets (uses CLIPSYNC_PACKAGE_FIXTURE_PREFIX).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

ROOT="$(clipsync_package_project_root)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/clipsync-package-test.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

PASS=0
FAIL=0

assert_eq() {
  local name="$1" got="$2" want="$3"
  if [[ "$got" == "$want" ]]; then
    echo "PASS: $name"
    PASS=$((PASS + 1))
  else
    echo "FAIL: $name (got='$got' want='$want')" >&2
    FAIL=$((FAIL + 1))
  fi
}

assert_ok() {
  local name="$1"
  shift
  if "$@"; then
    echo "PASS: $name"
    PASS=$((PASS + 1))
  else
    echo "FAIL: $name" >&2
    FAIL=$((FAIL + 1))
  fi
}

assert_fail() {
  local name="$1"
  shift
  if "$@"; then
    echo "FAIL: $name (expected failure)" >&2
    FAIL=$((FAIL + 1))
  else
    echo "PASS: $name"
    PASS=$((PASS + 1))
  fi
}

assert_contains() {
  local name="$1" hay="$2" needle="$3"
  if [[ "$hay" == *"$needle"* ]]; then
    echo "PASS: $name"
    PASS=$((PASS + 1))
  else
    echo "FAIL: $name (missing '$needle' in '$hay')" >&2
    FAIL=$((FAIL + 1))
  fi
}

# --- desktop escaping: two-layer + %% + newline reject ---
got="$(clipsync_desktop_escape_exec_arg '/home/user/My Apps/clipsync-desktop')"
want='"/home/user/My Apps/clipsync-desktop"'
assert_eq "escape spaces" "$got" "$want"

# Literal backslash requires four backslashes in the desktop file.
got="$(clipsync_desktop_escape_exec_arg '/tmp/foo\bar')"
want='"/tmp/foo\\\\bar"'
assert_eq "escape backslash two-layer" "$got" "$want"

# Dollar → \\$ in file (string \\ → \, then Exec \$ → $)
got="$(clipsync_desktop_escape_exec_arg '/tmp/a$b')"
want='"/tmp/a\\$b"'
assert_eq "escape dollar two-layer" "$got" "$want"

got="$(clipsync_desktop_escape_exec_arg '/tmp/a`b"c')"
want='"/tmp/a\\`b\\"c"'
assert_eq "escape backtick and quote" "$got" "$want"

got="$(clipsync_desktop_escape_exec_arg '/tmp/100% done/app')"
want='"/tmp/100%% done/app"'
assert_eq "escape percent fieldcode" "$got" "$want"

assert_fail "reject newline in path" \
  clipsync_desktop_escape_exec_arg $'/tmp/bad\nname'
assert_fail "reject equals in path" \
  clipsync_desktop_escape_exec_arg '/tmp/bad=name'

# Round-trip via unescape helper
for sample in \
  '/home/user/My Apps/clipsync-desktop' \
  '/tmp/foo\bar' \
  '/tmp/a$b`c"d' \
  '/tmp/100% done/app'
do
  enc="$(clipsync_desktop_escape_exec_arg "$sample")"
  dec="$(clipsync_desktop_unescape_exec_arg "$enc")"
  assert_eq "roundtrip $sample" "$dec" "$sample"
done

# --- dangerous force path guard ---
assert_ok "dangerous path detect root" clipsync_package_path_is_dangerous "/"
assert_fail "normal dist path not dangerous" clipsync_package_path_is_dangerous "$TMP/dist/out"

# --- fixture binaries ---
FIX_CLI="$TMP/fake-cli"
FIX_GUI="$TMP/fake-gui"
printf '#!/bin/sh\necho fixture-cli\n' >"$FIX_CLI"
printf '#!/bin/sh\necho fixture-gui\n' >"$FIX_GUI"
chmod 755 "$FIX_CLI" "$FIX_GUI"

[[ -f "$ROOT/assets/clipsync.svg" ]] || {
  echo "FAIL: assets/clipsync.svg missing" >&2
  exit 1
}

export CLIPSYNC_PACKAGE_DIST="$TMP/dist"
export CLIPSYNC_PACKAGE_SKIP_BUILD=1
export CLIPSYNC_PACKAGE_FIXTURE_MODE=1
export CLIPSYNC_PACKAGE_CLI_BIN="$FIX_CLI"
export CLIPSYNC_PACKAGE_GUI_BIN="$FIX_GUI"
export CLIPSYNC_PACKAGE_FORCE=0

# --- refuse overwrite ---
mkdir -p "$CLIPSYNC_PACKAGE_DIST/linux"
EXISTING="$CLIPSYNC_PACKAGE_DIST/linux/clipsync-companion-$(clipsync_package_version)-$(clipsync_package_arch)"
mkdir -p "$EXISTING"
assert_fail "refuse overwrite existing linux bundle" \
  bash "$SCRIPT_DIR/build-linux-companion.sh"

rm -rf "$CLIPSYNC_PACKAGE_DIST"
export CLIPSYNC_PACKAGE_FORCE=1

# --- linux assemble + verify + install into fixture prefix (not real HOME) ---
assert_ok "build-linux with fixtures" bash "$SCRIPT_DIR/build-linux-companion.sh"

BUNDLE="$(find "$CLIPSYNC_PACKAGE_DIST/linux" -maxdepth 1 -type d -name 'clipsync-companion-*' | head -1)"
[[ -n "$BUNDLE" ]]
assert_ok "verify linux bundle" bash "$SCRIPT_DIR/verify.sh" --linux "$BUNDLE"
assert_ok "provenance fixture_mode" grep -q '"fixture_mode": true' "$BUNDLE/share/doc/clipsync/PROVENANCE.json"
assert_ok "features not verified in fixture" grep -q '"verified": false' "$BUNDLE/share/doc/clipsync/PROVENANCE.json"
assert_ok "no relative from-bundle desktop" test ! -e "$BUNDLE/ClipSync-from-bundle.desktop"
assert_ok "install desktop Terminal=false" grep -q '^Terminal=false' "$BUNDLE/ClipSync-Install.desktop"
assert_ok "install desktop uses %k" grep -q '%k' "$BUNDLE/ClipSync-Install.desktop"
assert_fail "install desktop must not set Path=" \
  grep -qE '^Path=' "$BUNDLE/ClipSync-Install.desktop"
assert_fail "install desktop must not bake abs script argv" \
  grep -qE '^Exec=/bin/sh[[:space:]]+"?/' "$BUNDLE/ClipSync-Install.desktop"

FIXTURE_PREFIX="$TMP/fixture-home"
mkdir -p "$FIXTURE_PREFIX"
export CLIPSYNC_PACKAGE_FIXTURE_PREFIX="$FIXTURE_PREFIX"
export CLIPSYNC_INSTALL_PREFIX="$FIXTURE_PREFIX/.local"
export XDG_DATA_HOME="$FIXTURE_PREFIX/.local/share"
# Keep real HOME untouched for install targets; only fixture prefix is used.
REAL_HOME="${HOME}"
unset CLIPSYNC_INSTALL_DIR || true

assert_ok "user install into fixture prefix" bash "$BUNDLE/install-user.sh"
assert_ok "installed cli" test -x "$FIXTURE_PREFIX/.local/bin/clipsync"
assert_ok "installed gui sibling" test -x "$FIXTURE_PREFIX/.local/bin/clipsync-desktop"
assert_ok "installed desktop file" test -f "$FIXTURE_PREFIX/.local/share/applications/clipsync.desktop"
assert_ok "installed icon" test -f "$FIXTURE_PREFIX/.local/share/icons/hicolor/scalable/apps/clipsync.svg"
assert_eq "real HOME unchanged by install vars" "$HOME" "$REAL_HOME"

DESKTOP_EXEC_LINE="$(grep '^Exec=' "$FIXTURE_PREFIX/.local/share/applications/clipsync.desktop")"
assert_ok "Exec has no \$(" grep -qv '\$(' <<<"$DESKTOP_EXEC_LINE"
assert_ok "Exec has no backticks" grep -qv '`' <<<"$DESKTOP_EXEC_LINE"
assert_ok "Exec references installed gui" grep -q 'clipsync-desktop' <<<"$DESKTOP_EXEC_LINE"
assert_ok "Exec is absolute" grep -qE '^Exec="/' <<<"$DESKTOP_EXEC_LINE" || grep -qE '^Exec=/' <<<"$DESKTOP_EXEC_LINE"

# Decode Exec and compare to installed path
ENC="${DESKTOP_EXEC_LINE#Exec=}"
DEC="$(clipsync_desktop_unescape_exec_arg "$ENC")"
assert_eq "Exec decodes to installed gui" "$DEC" "$(realpath "$FIXTURE_PREFIX/.local/bin/clipsync-desktop" 2>/dev/null || echo "$FIXTURE_PREFIX/.local/bin/clipsync-desktop")"

# --- gio launch argv capture (absolute Exec; spaces in path) ---
# Round-trip tests above already cover %/$/`/" in Exec escaping. gio launch is
# sensitive to some Exec field-code edge cases, so the live argv capture uses a
# spaces-only absolute path (still CWD-independent).
ARGV_CAP="$TMP/argv-capture"
mkdir -p "$TMP/gio-bin"
CAP_BIN="$TMP/gio-bin/capture-argv"
cat >"$CAP_BIN" <<'EOS'
#!/bin/sh
# Desktop Exec with no args invokes the program as argv0 only; shell $@ is empty.
printf '%s\0' "$0" "$@" >"${CLIPSYNC_ARGV_OUT}"
EOS
chmod 755 "$CAP_BIN"

SPACE_DIR="$TMP/dir with spaces"
mkdir -p "$SPACE_DIR"
SPACE_BIN="$SPACE_DIR/capture-argv"
cp "$CAP_BIN" "$SPACE_BIN"
chmod 755 "$SPACE_BIN"
SPACE_ESC="$(clipsync_desktop_escape_exec_arg "$SPACE_BIN")"
GIO_DESKTOP="$TMP/gio-test.desktop"
cat >"$GIO_DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=ClipSync Gio Test
Exec=${SPACE_ESC}
Terminal=false
EOF

export CLIPSYNC_ARGV_OUT="$ARGV_CAP"
GIO_ARGV_PROVED=0
wait_for_file() {
  local path="$1" seconds="${2:-5}"
  local -i i
  for ((i = 0; i < seconds * 10; i++)); do
    [[ -e "$path" && -s "$path" ]] && return 0
    sleep 0.1
  done
  return 1
}
if command -v gio >/dev/null 2>&1; then
  chmod 755 "$GIO_DESKTOP" 2>/dev/null || true
  rm -f "$ARGV_CAP"
  if gio launch "$GIO_DESKTOP" 2>"$TMP/gio.err" && wait_for_file "$ARGV_CAP" 5; then
    GOT_ARGV="$(tr '\0' '\n' <"$ARGV_CAP" | head -1)"
    assert_eq "gio launch argv0 matches spaced path" "$GOT_ARGV" "$SPACE_BIN"
    GIO_ARGV_PROVED=1
  else
    echo "INFO: gio launch not usable on this host ($(uname -s)); recorded skip — unescape roundtrip still enforced"
    echo "gio.err: $(head -c 200 "$TMP/gio.err" 2>/dev/null || true)"
    assert_ok "gio skip recorded honestly" true
  fi
else
  assert_ok "gio absent — roundtrip-only Exec proof" true
fi

# --- terminal-free installer from an arbitrary extract directory ---
# Portable default uses %k (file-manager / GLib DesktopAppInfo). Stock gio launch
# on glib 2.74 does not expand %k — prove FM path via DesktopAppInfo.launch, and
# prove gio CLI via write-install-launcher.sh --absolute then gio launch.
ARBITRARY="$TMP/arbitrary extract dir with spaces/subdir"
mkdir -p "$ARBITRARY"
export CLIPSYNC_PACKAGE_FORCE=1
rm -rf "$CLIPSYNC_PACKAGE_DIST"
assert_ok "rebuild linux for arbitrary-dir activation" bash "$SCRIPT_DIR/build-linux-companion.sh"
BUNDLE_ARB="$(find "$CLIPSYNC_PACKAGE_DIST/linux" -maxdepth 1 -type d -name 'clipsync-companion-*' | head -1)"
cp -R "$BUNDLE_ARB"/* "$ARBITRARY/"
INSTALL_DESKTOP="$ARBITRARY/ClipSync-Install.desktop"
assert_ok "install desktop present after copy" test -f "$INSTALL_DESKTOP"
assert_ok "install desktop %k after copy" grep -q '%k' "$INSTALL_DESKTOP"

if command -v desktop-file-validate >/dev/null 2>&1; then
  assert_ok "desktop-file-validate ClipSync-Install.desktop" \
    desktop-file-validate "$INSTALL_DESKTOP"
else
  echo "INFO: desktop-file-validate not installed; skip structural validate"
  assert_ok "desktop-file-validate absent — skip recorded" true
fi

FIXTURE_PREFIX2="$TMP/fixture-home-fm"
mkdir -p "$FIXTURE_PREFIX2"
export CLIPSYNC_PACKAGE_FIXTURE_PREFIX="$FIXTURE_PREFIX2"
export CLIPSYNC_INSTALL_PREFIX="$FIXTURE_PREFIX2/.local"
export XDG_DATA_HOME="$FIXTURE_PREFIX2/.local/share"
export CLIPSYNC_INSTALL_DESKTOP="$INSTALL_DESKTOP"
FM_CWD="$TMP/unrelated cwd"
mkdir -p "$FM_CWD"

# File-manager equivalent: GLib DesktopAppInfo.launch (expands %k).
if [[ "$(uname -s)" == "Linux" ]] && command -v python3 >/dev/null 2>&1; then
  set +e
  python3 - <<'PY' 2>"$TMP/fm-launch.err"
import os, sys
try:
    import gi
    gi.require_version("Gio", "2.0")
    from gi.repository import Gio
except Exception as e:
    print("gi_unavailable", e)
    sys.exit(2)
path = os.environ["CLIPSYNC_INSTALL_DESKTOP"]
info = Gio.DesktopAppInfo.new_from_filename(path)
if info is None:
    sys.exit(3)
ok = info.launch([], None)
sys.exit(0 if ok else 4)
PY
  FM_RC=$?
  set -e
  if [[ "$FM_RC" -eq 0 ]]; then
    # DesktopAppInfo.launch returns before the installer child finishes.
    if wait_for_file "$FIXTURE_PREFIX2/.local/share/applications/clipsync.desktop" 10 \
      && wait_for_file "$FIXTURE_PREFIX2/.local/bin/clipsync-desktop" 10; then
      assert_ok "DesktopAppInfo launch install from arbitrary dir" \
        test -x "$FIXTURE_PREFIX2/.local/bin/clipsync-desktop"
      assert_ok "FM install wrote desktop entry" \
        test -f "$FIXTURE_PREFIX2/.local/share/applications/clipsync.desktop"
      assert_eq "FM install left real HOME unchanged" "$HOME" "$REAL_HOME"
    else
      echo "FAIL: DesktopAppInfo.launch started but install artifacts never appeared" >&2
      FAIL=$((FAIL + 1))
    fi
  elif [[ "$FM_RC" -eq 2 ]]; then
    echo "fm-launch.err: $(head -c 300 "$TMP/fm-launch.err" 2>/dev/null || true)"
    assert_ok "python gi unavailable — FM launch skip recorded" true
  else
    echo "fm-launch.err: $(head -c 300 "$TMP/fm-launch.err" 2>/dev/null || true)"
    echo "FAIL: DesktopAppInfo.launch of portable ClipSync-Install.desktop failed (rc=$FM_RC)" >&2
    FAIL=$((FAIL + 1))
  fi
else
  assert_ok "non-Linux or no python3 — FM launch not proved here" true
fi

# gio launch CLI requires absolute Exec on glib 2.74 (no %k expansion).
FIXTURE_PREFIX3="$TMP/fixture-home-gio-abs"
mkdir -p "$FIXTURE_PREFIX3"
export CLIPSYNC_PACKAGE_FIXTURE_PREFIX="$FIXTURE_PREFIX3"
export CLIPSYNC_INSTALL_PREFIX="$FIXTURE_PREFIX3/.local"
export XDG_DATA_HOME="$FIXTURE_PREFIX3/.local/share"
assert_ok "rewrite absolute install desktop for gio CLI" \
  bash "$ARBITRARY/write-install-launcher.sh" --absolute
assert_fail "absolute mode must not keep %k" grep -q '%k' "$INSTALL_DESKTOP"
if command -v desktop-file-validate >/dev/null 2>&1; then
  assert_ok "desktop-file-validate absolute install desktop" \
    desktop-file-validate "$INSTALL_DESKTOP"
fi
if command -v gio >/dev/null 2>&1; then
  if ( cd "$FM_CWD" && gio launch "$INSTALL_DESKTOP" ) 2>"$TMP/gio-abs.err" \
    && wait_for_file "$FIXTURE_PREFIX3/.local/bin/clipsync-desktop" 10; then
    assert_ok "gio launch absolute install from arbitrary dir" \
      test -x "$FIXTURE_PREFIX3/.local/bin/clipsync-desktop"
    assert_eq "gio abs install left real HOME unchanged" "$HOME" "$REAL_HOME"
  else
    echo "gio-abs.err: $(head -c 300 "$TMP/gio-abs.err" 2>/dev/null || true)"
    if [[ "$(uname -s)" == "Linux" ]]; then
      echo "FAIL: gio launch with --absolute Install.desktop must work on Linux" >&2
      FAIL=$((FAIL + 1))
    else
      assert_ok "gio abs skip on non-Linux" true
    fi
  fi
else
  assert_ok "gio absent — absolute install path not proved" true
fi

# Restore portable desktop + primary fixture prefix for subsequent upgrade tests.
bash "$ARBITRARY/write-install-launcher.sh" --portable >/dev/null
export CLIPSYNC_PACKAGE_FIXTURE_PREFIX="$FIXTURE_PREFIX"
export CLIPSYNC_INSTALL_PREFIX="$FIXTURE_PREFIX/.local"
export XDG_DATA_HOME="$FIXTURE_PREFIX/.local/share"

# --- upgrade keeps .previous; inject GUI failure → CLI rollback ---
printf '#!/bin/sh\necho older-cli\n' >"$FIXTURE_PREFIX/.local/bin/clipsync"
printf '#!/bin/sh\necho older-gui\n' >"$FIXTURE_PREFIX/.local/bin/clipsync-desktop"
chmod 755 "$FIXTURE_PREFIX/.local/bin/clipsync" "$FIXTURE_PREFIX/.local/bin/clipsync-desktop"
# Seed config/trust/history markers that must survive failed upgrade
mkdir -p "$FIXTURE_PREFIX/.config/clipsync"
echo trust-marker >"$FIXTURE_PREFIX/.config/clipsync/trust-marker"
echo history-marker >"$FIXTURE_PREFIX/.config/clipsync/history-marker"

# Inject GUI failure after CLI swap; both .previous retained; CLI rolled back.
export CLIPSYNC_PACKAGE_FIXTURE_PREFIX="$FIXTURE_PREFIX"
CLI_BEFORE="$(cat "$FIXTURE_PREFIX/.local/bin/clipsync")"
GUI_BEFORE="$(cat "$FIXTURE_PREFIX/.local/bin/clipsync-desktop")"
assert_ok "pair rollback on gui failure" \
  env CLIPSYNC_PACKAGE_INJECT_PAIR_FAIL=gui bash -c '
    set -euo pipefail
    source "'"$SCRIPT_DIR"'/common.sh"
    src_cli="'"$FIX_CLI"'"
    src_gui="'"$FIX_GUI"'"
    dst_cli="'"$FIXTURE_PREFIX"'/.local/bin/clipsync"
    dst_gui="'"$FIXTURE_PREFIX"'/.local/bin/clipsync-desktop"
    if clipsync_package_install_binary_pair_safe "$src_cli" "$dst_cli" "$src_gui" "$dst_gui"; then
      exit 1
    fi
    exit 0
  '
CLI_AFTER="$(cat "$FIXTURE_PREFIX/.local/bin/clipsync")"
GUI_AFTER="$(cat "$FIXTURE_PREFIX/.local/bin/clipsync-desktop")"
assert_eq "CLI restored after GUI failure" "$CLI_AFTER" "$CLI_BEFORE"
assert_eq "GUI unchanged after injected failure" "$GUI_AFTER" "$GUI_BEFORE"
assert_ok "previous CLI retained for repair" test -f "$FIXTURE_PREFIX/.local/bin/clipsync.previous"
assert_ok "previous GUI retained for repair" test -f "$FIXTURE_PREFIX/.local/bin/clipsync-desktop.previous"
assert_ok "trust marker preserved" grep -q trust-marker "$FIXTURE_PREFIX/.config/clipsync/trust-marker"
assert_ok "history marker preserved" grep -q history-marker "$FIXTURE_PREFIX/.config/clipsync/history-marker"

# Successful upgrade retains .previous
printf '#!/bin/sh\necho newer\n' >"$FIX_GUI"
printf '#!/bin/sh\necho newer-cli\n' >"$FIX_CLI"
export CLIPSYNC_PACKAGE_FORCE=1
rm -rf "$CLIPSYNC_PACKAGE_DIST"
assert_ok "rebuild linux for upgrade" bash "$SCRIPT_DIR/build-linux-companion.sh"
BUNDLE2="$(find "$CLIPSYNC_PACKAGE_DIST/linux" -maxdepth 1 -type d -name 'clipsync-companion-*' | head -1)"
assert_ok "upgrade install keeps previous" bash "$BUNDLE2/install-user.sh"
assert_ok "previous gui retained" test -f "$FIXTURE_PREFIX/.local/bin/clipsync-desktop.previous"
assert_ok "previous cli retained" test -f "$FIXTURE_PREFIX/.local/bin/clipsync.previous"

# No host systemd unit paths under fixture prefix
assert_ok "no systemd unit by installer" \
  test ! -e "$FIXTURE_PREFIX/.config/systemd/user/clipsync.service"

# --- host format guard: Mach-O must not pass linux require without fixture ---
if [[ "$(uname -s)" == "Darwin" ]]; then
  # Pick any Mach-O on PATH (e.g. /bin/ls)
  assert_fail "linux format rejects Mach-O outside fixture" \
    env -u CLIPSYNC_PACKAGE_FIXTURE_MODE -u CLIPSYNC_PACKAGE_SKIP_BUILD \
      bash -c 'source "'"$SCRIPT_DIR"'/common.sh"; clipsync_package_require_binary_format /bin/ls linux'
  assert_ok "fixture mode allows script for linux checks" \
    env CLIPSYNC_PACKAGE_FIXTURE_MODE=1 \
      bash -c 'source "'"$SCRIPT_DIR"'/common.sh"; clipsync_package_require_binary_format "'"$FIX_CLI"'" linux'
fi

# --- macOS assemble when on Darwin ---
if [[ "$(uname -s)" == "Darwin" ]]; then
  export CLIPSYNC_PACKAGE_FORCE=1
  export CLIPSYNC_PACKAGE_DIST="$TMP/dist-mac"
  export CLIPSYNC_PACKAGE_FIXTURE_MODE=1
  export CLIPSYNC_PACKAGE_SKIP_BUILD=1
  assert_ok "build-macos with fixtures" bash "$SCRIPT_DIR/build-macos-companion.sh"
  APP="$CLIPSYNC_PACKAGE_DIST/macos/ClipSync.app"
  assert_ok "verify macos app" bash "$SCRIPT_DIR/verify.sh" --macos "$APP"
  assert_ok "sibling cli in MacOS" test -x "$APP/Contents/MacOS/clipsync"
  assert_ok "gui executable name" test -x "$APP/Contents/MacOS/clipsync-desktop"
  DMG="$(find "$CLIPSYNC_PACKAGE_DIST/macos" -name '*UNSIGNED-DEVELOPMENT.dmg' | head -1)"
  assert_ok "unsigned DMG labeled" test -n "$DMG"
  if [[ -f "$APP/Contents/Resources/AppIcon.icns" ]]; then
    assert_ok "icns embedded from svg" true
    assert_ok "plist references icon" grep -q 'CFBundleIconFile' "$APP/Contents/Info.plist"
  else
    echo "INFO: icns conversion skipped on this host"
    assert_ok "icns optional when converters fail" true
  fi
fi

# --- sign gate without credentials ---
mkdir -p "$TMP/dummy.app/Contents/MacOS"
assert_fail "sign blocked missing identity" \
  env -u CLIPSYNC_CODESIGN_IDENTITY -u CLIPSYNC_NOTARY_PROFILE \
    -u CLIPSYNC_NOTARY_APPLE_ID -u CLIPSYNC_NOTARY_TEAM_ID -u CLIPSYNC_NOTARY_PASSWORD \
  bash "$SCRIPT_DIR/sign-macos.sh" --app "$TMP/dummy.app"

# --- mocked sign/notarize flow: correct order + failed notary does not claim success ---
MOCK_BIN="$TMP/mock-bin"
mkdir -p "$MOCK_BIN" "$TMP/mock-app/Contents/MacOS" "$TMP/mock-app/Contents/Resources"
printf '#!/bin/sh\nexit 0\n' >"$TMP/mock-app/Contents/MacOS/clipsync"
printf '#!/bin/sh\nexit 0\n' >"$TMP/mock-app/Contents/MacOS/clipsync-desktop"
chmod 755 "$TMP/mock-app/Contents/MacOS/"*
echo '{"signing":"UNSIGNED-DEVELOPMENT","notarized":false,"public_release_ready":false}' \
  >"$TMP/mock-app/Contents/Resources/PROVENANCE.json"
MOCK_DMG="$TMP/mock-out/ClipSync.dmg"
mkdir -p "$TMP/mock-out"
# Pre-create an "old" DMG that must be replaced after staple
echo old-unsigned-dmg >"$MOCK_DMG"
MOCK_LOG="$TMP/mock-calls.log"
: >"$MOCK_LOG"

cat >"$MOCK_BIN/codesign" <<EOS
#!/usr/bin/env bash
echo "codesign \$*" >>"$MOCK_LOG"
if echo "\$*" | grep -q '\\.dmg' && [[ "\${MOCK_FAIL_DMG_CODESIGN:-0}" == "1" ]]; then
  exit 1
fi
exit 0
EOS
cat >"$MOCK_BIN/ditto" <<EOS
#!/usr/bin/env bash
echo "ditto \$*" >>"$MOCK_LOG"
out="\${*: -1}"
: >"\$out"
exit 0
EOS
cat >"$MOCK_BIN/xcrun" <<EOS
#!/usr/bin/env bash
echo "xcrun \$*" >>"$MOCK_LOG"
if [[ "\$1" == "notarytool" ]]; then
  if [[ "\${MOCK_FAIL_NOTARY:-0}" == "1" ]]; then
    echo "mock notary failure" >&2
    exit 1
  fi
  exit 0
fi
exit 0
EOS
cat >"$MOCK_BIN/hdiutil" <<EOS
#!/usr/bin/env bash
echo "hdiutil \$*" >>"$MOCK_LOG"
out="\${*: -1}"
echo "recreated-from-stapled-app" >"\$out"
exit 0
EOS
cat >"$MOCK_BIN/spctl" <<EOS
#!/usr/bin/env bash
echo "spctl \$*" >>"$MOCK_LOG"
exit 0
EOS
chmod 755 "$MOCK_BIN"/*

export MOCK_LOG
export CLIPSYNC_CODESIGN_IDENTITY="Developer ID Application: MOCK"
export CLIPSYNC_NOTARY_PROFILE="mock-profile"
export CLIPSYNC_PACKAGE_MOCK_BIN="$MOCK_BIN"

assert_ok "mock sign+notarize success flow" \
  bash "$SCRIPT_DIR/sign-macos.sh" --app "$TMP/mock-app" --dmg "$MOCK_DMG"

# acceptance filename is derived from app basename (mock-app → ACCEPTANCE-mock-app.json)
ACCEPT="$(dirname "$TMP/mock-app")/ACCEPTANCE-mock-app.json"
assert_ok "acceptance file written" test -f "$ACCEPT"
assert_ok "acceptance accepted" grep -q '"status": "accepted"' "$ACCEPT"
assert_ok "acceptance notarized true" grep -q '"notarized": true' "$ACCEPT"
assert_contains "flow zipped before notary" "$(cat "$MOCK_LOG")" "ditto"
assert_contains "flow submitted zip via notarytool" "$(cat "$MOCK_LOG")" "notarytool submit"
assert_contains "flow stapled app" "$(cat "$MOCK_LOG")" "stapler staple"
assert_contains "flow recreated dmg" "$(cat "$MOCK_LOG")" "hdiutil"
assert_ok "dmg content replaced from stapled app" grep -q recreated-from-stapled-app "$MOCK_DMG"
assert_ok "verify acceptance" bash "$SCRIPT_DIR/verify.sh" --acceptance "$ACCEPT"

# Failed notarization must not claim success
: >"$MOCK_LOG"
rm -f "$ACCEPT"
echo old-must-remain >"$MOCK_DMG"
export MOCK_FAIL_NOTARY=1
assert_fail "mock notary failure does not succeed" \
  bash "$SCRIPT_DIR/sign-macos.sh" --app "$TMP/mock-app" --dmg "$MOCK_DMG"
ACCEPT="$(dirname "$TMP/mock-app")/ACCEPTANCE-mock-app.json"
assert_ok "failed acceptance exists" test -f "$ACCEPT"
assert_ok "failed acceptance not release-ready" grep -q '"public_release_ready": false' "$ACCEPT"
assert_ok "failed acceptance notarized false" grep -q '"notarized": false' "$ACCEPT"
# App-zip notary fails before DMG recreate; destination DMG must remain untouched.
assert_ok "notary fail leaves prior DMG bytes" grep -q old-must-remain "$MOCK_DMG"
unset MOCK_FAIL_NOTARY

# DMG codesign failure is fatal (no || true) and must not replace destination DMG
: >"$MOCK_LOG"
rm -f "$ACCEPT"
echo old-dmg-before-codesign-fail >"$MOCK_DMG"
export MOCK_FAIL_NOTARY=0
export MOCK_FAIL_DMG_CODESIGN=1
assert_fail "dmg codesign failure is fatal" \
  bash "$SCRIPT_DIR/sign-macos.sh" --app "$TMP/mock-app" --dmg "$MOCK_DMG"
assert_ok "dmg codesign fail leaves prior DMG" grep -q old-dmg-before-codesign-fail "$MOCK_DMG"
unset MOCK_FAIL_DMG_CODESIGN
unset CLIPSYNC_CODESIGN_IDENTITY CLIPSYNC_NOTARY_PROFILE CLIPSYNC_PACKAGE_MOCK_BIN

# Note whether Linux gio argv capture was proved (for summary consumers).
if [[ "$GIO_ARGV_PROVED" -eq 1 ]]; then
  echo "INFO: gio launch argv capture PROVED on $(uname -s)"
fi

echo
echo "package tests: $PASS passed, $FAIL failed"
echo "NOTE: fixture packages are NOT real release/ELF/Mach-O proof."
[[ "$FAIL" -eq 0 ]]
