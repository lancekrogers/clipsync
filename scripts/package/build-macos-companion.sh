#!/usr/bin/env bash
# Assemble ClipSync.app + UNSIGNED development DMG (Apple Silicon / host arch).
# Does not sign or notarize. Existing scripts/install_user.sh paths are untouched.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

[[ "$(uname -s)" == "Darwin" ]] || clipsync_package_die "build-macos-companion.sh requires macOS"

ROOT="$(clipsync_package_project_root)"
VERSION="$(clipsync_package_version)"
ARCH="$(clipsync_package_arch)"
DIST="$(clipsync_package_dist_root)/macos"
TARGET_DIR="$(clipsync_package_target_dir)"
LABEL="$(clipsync_package_artifact_label_unsigned)"
SKIP_BUILD="${CLIPSYNC_PACKAGE_SKIP_BUILD:-0}"

APP_NAME="ClipSync.app"
APP_PATH="$DIST/$APP_NAME"
DMG_NAME="ClipSync-${VERSION}-${ARCH}-${LABEL}.dmg"
DMG_PATH="$DIST/$DMG_NAME"
STAGE="$DIST/dmg-stage"

mkdir -p "$DIST"

if [[ "$SKIP_BUILD" != "1" ]]; then
  bash "$SCRIPT_DIR/build-binaries.sh" >/dev/null
fi

CLI_BIN="${CLIPSYNC_PACKAGE_CLI_BIN:-$TARGET_DIR/release/clipsync}"
GUI_BIN="${CLIPSYNC_PACKAGE_GUI_BIN:-$TARGET_DIR/release/clipsync-desktop}"
[[ -f "$CLI_BIN" ]] || clipsync_package_die "CLI binary missing: $CLI_BIN"
[[ -f "$GUI_BIN" ]] || clipsync_package_die "GUI binary missing: $GUI_BIN"

clipsync_package_require_binary_format "$CLI_BIN" macos "$GUI_BIN" >/dev/null
clipsync_package_require_binary_format "$GUI_BIN" macos "$CLI_BIN" >/dev/null

clipsync_package_refuse_overwrite "$APP_PATH"
clipsync_package_refuse_overwrite "$DMG_PATH"

clipsync_package_log "assembling $APP_PATH"

MACOS_DIR="$APP_PATH/Contents/MacOS"
RESOURCES_DIR="$APP_PATH/Contents/Resources"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

cp "$GUI_BIN" "$MACOS_DIR/clipsync-desktop"
cp "$CLI_BIN" "$MACOS_DIR/clipsync"
chmod 755 "$MACOS_DIR/clipsync-desktop" "$MACOS_DIR/clipsync"

# Convert owned SVG → .icns via platform tools (rsvg-convert/sips/iconutil).
# Fail soft to a missing icon only when converters are absent; never invent pixels.
ICNS_OUT="$RESOURCES_DIR/AppIcon.icns"
SVG_SRC="$ROOT/assets/clipsync.svg"
build_icns_from_svg() {
  local svg="$1" icns="$2"
  local work iconset
  command -v iconutil >/dev/null 2>&1 || return 1
  work="$(mktemp -d "${TMPDIR:-/tmp}/clipsync-icns.XXXXXX")"
  iconset="$work/AppIcon.iconset"
  mkdir -p "$iconset"
  local png1024="$work/icon-1024.png"
  if command -v rsvg-convert >/dev/null 2>&1; then
    rsvg-convert -w 1024 -h 1024 "$svg" -o "$png1024" || { rm -rf "$work"; return 1; }
  elif command -v magick >/dev/null 2>&1; then
    magick -background none "$svg" -resize 1024x1024 "$png1024" || { rm -rf "$work"; return 1; }
  else
    rm -rf "$work"
    return 1
  fi
  # Standard iconset sizes
  local size name
  for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$png1024" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) "$png1024" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
  done
  if ! iconutil -c icns "$iconset" -o "$icns"; then
    rm -rf "$work"
    return 1
  fi
  rm -rf "$work"
  return 0
}

ICON_FILE_KEY=""
if [[ -f "$SVG_SRC" ]] && build_icns_from_svg "$SVG_SRC" "$ICNS_OUT"; then
  clipsync_package_log "embedded AppIcon.icns from assets/clipsync.svg"
  ICON_FILE_KEY="AppIcon"
  cp "$SVG_SRC" "$RESOURCES_DIR/clipsync.svg"
else
  clipsync_package_warn "icns conversion unavailable; shipping without CFBundleIconFile (generic app icon)"
fi

VERSION_PLIST="$(clipsync_plist_escape "$VERSION")"
LABEL_PLIST="$(clipsync_plist_escape "$LABEL")"

# No bundled SF fonts — runtime uses local system typography only (D01).
cat >"$APP_PATH/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>clipsync-desktop</string>
	<key>CFBundleIdentifier</key>
	<string>com.clipsync.desktop</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>ClipSync</string>
	<key>CFBundleDisplayName</key>
	<string>ClipSync</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>${VERSION_PLIST}</string>
	<key>CFBundleVersion</key>
	<string>${VERSION_PLIST}</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.utilities</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSSupportsAutomaticGraphicsSwitching</key>
	<true/>
	<key>CFBundleGetInfoString</key>
	<string>ClipSync ${VERSION_PLIST} (${LABEL_PLIST}). Not notarized. Local development only.</string>
EOF

if [[ -n "$ICON_FILE_KEY" ]]; then
  cat >>"$APP_PATH/Contents/Info.plist" <<EOF
	<key>CFBundleIconFile</key>
	<string>${ICON_FILE_KEY}</string>
EOF
fi

cat >>"$APP_PATH/Contents/Info.plist" <<EOF
</dict>
</plist>
EOF

clipsync_package_write_provenance "$APP_PATH/Contents/Resources/PROVENANCE.json" "macos-app"

cat >"$RESOURCES_DIR/README-UNSIGNED.txt" <<EOF
ClipSync companion — ${LABEL}

This .app is a local development artifact.
- Not Developer ID signed
- Not notarized
- Not Gatekeeper-ready for public distribution

Layout:
  Contents/MacOS/clipsync-desktop  (GUI; CFBundleExecutable)
  Contents/MacOS/clipsync          (matching CLI/daemon/TUI sibling)

The GUI resolves the daemon via the sibling clipsync binary in the same
MacOS directory (see clipsync::companion default_daemon_binary).

Uninstall: move ClipSync.app to Trash. User config, identity, trust cache,
authorized peers, history database and history key are NOT removed.
EOF

# DMG stage
clipsync_package_refuse_overwrite "$STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp -R "$APP_PATH" "$STAGE/"

cat >"$STAGE/README-${LABEL}.txt" <<EOF
ClipSync ${VERSION} (${ARCH}) — ${LABEL}

This disk image is NOT a public release.
Signing/notarization requires Developer ID credentials (see just companion-sign-macos).

Install (development):
1. Drag ClipSync.app to /Applications (or ~/Applications).
2. First launch may be blocked by Gatekeeper for unsigned apps — expected for this artifact.
3. Open the app; terminal-free setup uses the shared Companion flow.
4. CLI/TUI: ClipSync.app/Contents/MacOS/clipsync  (or install CLI via existing user scripts).

Do not claim this DMG is notarized or release-ready.
EOF

ln -s /Applications "$STAGE/Applications"

clipsync_package_log "creating DMG $DMG_PATH"
hdiutil create \
  -volname "ClipSync ${VERSION} ${LABEL}" \
  -srcfolder "$STAGE" \
  -ov -format UDZO \
  "$DMG_PATH"

rm -rf "$STAGE"

VERSION_SAFE="$(clipsync_json_escape "$VERSION")"
ARCH_SAFE="$(clipsync_json_escape "$ARCH")"
LABEL_SAFE="$(clipsync_json_escape "$LABEL")"
FIXTURE_JSON=false
FEATURES_VERIFIED=true
if clipsync_package_fixture_mode; then
  FIXTURE_JSON=true
  FEATURES_VERIFIED=false
fi

ICON_JSON="null"
if [[ -n "$ICON_FILE_KEY" ]]; then
  ICON_JSON="\"Contents/Resources/AppIcon.icns\""
fi

MANIFEST="$DIST/MANIFEST-${VERSION}-${ARCH}.json"
cat >"$MANIFEST" <<EOF
{
  "version": "$VERSION_SAFE",
  "arch": "$ARCH_SAFE",
  "signing": "$LABEL_SAFE",
  "notarized": false,
  "public_release_ready": false,
  "fixture_mode": $FIXTURE_JSON,
  "features_verified": $FEATURES_VERIFIED,
  "app": "$(clipsync_json_escape "$APP_NAME")",
  "dmg": "$(clipsync_json_escape "$DMG_NAME")",
  "macos_layout": {
    "executable": "Contents/MacOS/clipsync-desktop",
    "sibling_cli": "Contents/MacOS/clipsync",
    "icon": $ICON_JSON
  }
}
EOF

clipsync_package_log "macOS package complete (unsigned development)"
clipsync_package_log "  app: $APP_PATH"
clipsync_package_log "  dmg: $DMG_PATH"
clipsync_package_log "  manifest: $MANIFEST"
if clipsync_package_fixture_mode; then
  clipsync_package_log "  FIXTURE MODE — not a verified Mach-O release proof"
fi
