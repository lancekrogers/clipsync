#!/usr/bin/env bash
# Optional Developer ID sign + notarize. Hard-gated on explicit credentials.
# Default packaging never calls this. No identity is selected implicitly.
#
# Correct order (Apple direct distribution):
#   1) codesign nested Mach-O then .app (hardened runtime)
#   2) zip the .app (notarytool cannot submit a bare .app folder)
#   3) notarytool submit zip --wait
#   4) stapler staple .app
#   5) recreate DMG from the signed+stapled .app (old DMG still has unsigned bytes)
#   6) codesign DMG (failure is fatal — never `|| true`)
#   7) notarize + staple DMG
#   8) write ACCEPTANCE.json; only then may public_release_ready be considered
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

usage() {
  cat <<EOF
Usage: sign-macos.sh --app PATH [--dmg PATH]

Required environment (all must be set; none are available in the default worker env):
  CLIPSYNC_CODESIGN_IDENTITY   Developer ID Application identity string
  CLIPSYNC_NOTARY_PROFILE      notarytool keychain profile name
                               OR set CLIPSYNC_NOTARY_APPLE_ID + CLIPSYNC_NOTARY_TEAM_ID
                                  + CLIPSYNC_NOTARY_PASSWORD (app-specific)

This script refuses to run when credentials are missing. It never falls back to
ad-hoc signing or claims notarization succeeded without notarytool acceptance.

Mock/test hook: set CLIPSYNC_PACKAGE_MOCK_BIN to a directory of stub tools
(codesign, notarytool/xcrun, stapler, ditto, hdiutil, spctl) prepended to PATH.
EOF
}

APP=""
DMG=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) APP="${2:-}"; shift 2 ;;
    --dmg) DMG="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) clipsync_package_die "unknown arg: $1" ;;
  esac
done

[[ -n "$APP" ]] || { usage; exit 2; }
[[ -d "$APP" ]] || clipsync_package_die "app bundle not found: $APP"

if [[ -n "${CLIPSYNC_PACKAGE_MOCK_BIN:-}" ]]; then
  export PATH="${CLIPSYNC_PACKAGE_MOCK_BIN}:$PATH"
fi

if [[ -z "${CLIPSYNC_CODESIGN_IDENTITY:-}" ]]; then
  cat >&2 <<EOF
sign-macos.sh: blocked — CLIPSYNC_CODESIGN_IDENTITY is not set.

No Developer ID Application identity is assumed. Local unsigned development
artifacts from build-macos-companion.sh remain the supported path in this environment.
Public notarized release is not release-ready until credentials are provisioned
(see D05). Refusing to sign with an implicit or ad-hoc identity.
EOF
  exit 3
fi

if [[ -z "${CLIPSYNC_NOTARY_PROFILE:-}" \
   && ( -z "${CLIPSYNC_NOTARY_APPLE_ID:-}" || -z "${CLIPSYNC_NOTARY_TEAM_ID:-}" || -z "${CLIPSYNC_NOTARY_PASSWORD:-}" ) ]]; then
  cat >&2 <<EOF
sign-macos.sh: blocked — notarization credentials incomplete.

Set CLIPSYNC_NOTARY_PROFILE or CLIPSYNC_NOTARY_APPLE_ID + CLIPSYNC_NOTARY_TEAM_ID
+ CLIPSYNC_NOTARY_PASSWORD. Without notarization credentials this script will not
produce a Gatekeeper-ready artifact or claim public release readiness.
EOF
  exit 3
fi

IDENTITY="$CLIPSYNC_CODESIGN_IDENTITY"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/clipsync-sign.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

ACCEPTANCE="$(dirname "$APP")/ACCEPTANCE-$(basename "$APP" .app).json"
FLOW_LOG="$WORK/flow.log"
: >"$FLOW_LOG"
log_step() {
  printf '%s\n' "$1" | tee -a "$FLOW_LOG" >/dev/null
  clipsync_package_log "$1"
}

write_acceptance() {
  local status="$1" detail="$2"
  local notarized=false ready=false
  if [[ "$status" == "accepted" ]]; then
    notarized=true
    ready=true
  fi
  cat >"$ACCEPTANCE" <<EOF
{
  "app": "$(clipsync_json_escape "$APP")",
  "dmg": "$(clipsync_json_escape "${DMG:-}")",
  "status": "$(clipsync_json_escape "$status")",
  "detail": "$(clipsync_json_escape "$detail")",
  "notarized": $notarized,
  "public_release_ready": $ready,
  "signing": "$( [[ "$status" == "accepted" ]] && echo Developer-ID || echo UNSIGNED-OR-FAILED )",
  "flow": $(python3 -c 'import json,sys; print(json.dumps(open(sys.argv[1]).read().splitlines()))' "$FLOW_LOG" 2>/dev/null || echo '[]')
}
EOF
}

fail_acceptance() {
  local msg="$1"
  write_acceptance "failed" "$msg"
  clipsync_package_die "$msg"
}

log_step "codesign-nested-cli"
codesign --force --options runtime --timestamp \
  --sign "$IDENTITY" \
  "$APP/Contents/MacOS/clipsync"

log_step "codesign-nested-gui"
codesign --force --options runtime --timestamp \
  --sign "$IDENTITY" \
  "$APP/Contents/MacOS/clipsync-desktop"

log_step "codesign-app-bundle"
codesign --force --options runtime --timestamp --deep \
  --sign "$IDENTITY" \
  "$APP"

log_step "codesign-verify-app"
codesign --verify --deep --strict "$APP"

# notarytool cannot submit a .app directory; zip it first.
APP_ZIP="$WORK/ClipSync-submit.zip"
log_step "zip-app-for-notary"
# ditto preserves bundle metadata better than zip for notarization submissions.
ditto -c -k --keepParent "$APP" "$APP_ZIP"

submit_notary() {
  local path="$1"
  if [[ -n "${CLIPSYNC_NOTARY_PROFILE:-}" ]]; then
    xcrun notarytool submit "$path" --keychain-profile "$CLIPSYNC_NOTARY_PROFILE" --wait
  else
    xcrun notarytool submit "$path" \
      --apple-id "$CLIPSYNC_NOTARY_APPLE_ID" \
      --team-id "$CLIPSYNC_NOTARY_TEAM_ID" \
      --password "$CLIPSYNC_NOTARY_PASSWORD" \
      --wait
  fi
}

log_step "notarytool-submit-app-zip"
if ! submit_notary "$APP_ZIP"; then
  fail_acceptance "notarytool rejected or failed for app zip; not claiming signed success"
fi

log_step "stapler-staple-app"
xcrun stapler staple "$APP"

# Recreate DMG from the signed+stapled app so the disk image does not embed the
# old unsigned bundle. Signing an existing DMG does not fix nested contents.
#
# Atomicity: build+sign+notary+staple a temp DMG first; only replace the
# destination path after success. A codesign/notary failure must leave any
# pre-existing DMG untouched (no rm-then-fail hole).
if [[ -n "$DMG" ]]; then
  STAGE="$WORK/dmg-stage"
  mkdir -p "$STAGE"
  cp -R "$APP" "$STAGE/"
  ln -sf /Applications "$STAGE/Applications"
  TMP_DMG="$WORK/ClipSync-stapled.dmg"
  log_step "recreate-dmg-from-stapled-app"
  if clipsync_package_path_is_dangerous "$DMG"; then
    fail_acceptance "refusing to replace dangerous DMG path: $DMG"
  fi
  hdiutil create \
    -volname "ClipSync" \
    -srcfolder "$STAGE" \
    -ov -format UDZO \
    "$TMP_DMG"

  log_step "codesign-dmg"
  # DMG codesign failure is fatal — never ignored. Old $DMG still intact.
  codesign --force --sign "$IDENTITY" "$TMP_DMG"

  log_step "notarytool-submit-dmg"
  if ! submit_notary "$TMP_DMG"; then
    fail_acceptance "notarytool rejected or failed for DMG; app may be stapled but destination DMG was not replaced"
  fi

  log_step "stapler-staple-dmg"
  xcrun stapler staple "$TMP_DMG"

  log_step "publish-dmg-replace"
  mkdir -p "$(dirname "$DMG")"
  mv -f "$TMP_DMG" "$DMG"
fi

log_step "spctl-assess-app"
if command -v spctl >/dev/null 2>&1; then
  # Assessment may fail in mock mode; record but only soft-warn when MOCK is set.
  if spctl --assess --type execute --verbose "$APP" 2>&1 | tee -a "$FLOW_LOG"; then
    log_step "spctl-assess-ok"
  else
    if [[ -n "${CLIPSYNC_PACKAGE_MOCK_BIN:-}" ]]; then
      log_step "spctl-assess-skipped-or-failed-under-mock"
    else
      fail_acceptance "spctl assess failed after notarization"
    fi
  fi
fi

write_acceptance "accepted" "Developer ID signed, notarized, stapled; DMG recreated from stapled app when requested"

# Update adjacent provenance if present.
PROV="$APP/Contents/Resources/PROVENANCE.json"
if [[ -f "$PROV" ]]; then
  python3 - "$PROV" <<'PY' 2>/dev/null || true
import json,sys
p=sys.argv[1]
with open(p) as f: d=json.load(f)
d["signing"]="Developer-ID"
d["notarized"]=True
d["public_release_ready"]=True
d["notes"]="Signed and notarized via sign-macos.sh. Verify ACCEPTANCE.json + stapler + spctl before distribution."
with open(p,"w") as f: json.dump(d,f,indent=2); f.write("\n")
PY
fi

clipsync_package_log "notarization + staple complete"
clipsync_package_log "acceptance: $ACCEPTANCE"
if [[ -n "$DMG" ]]; then
  clipsync_package_log "dmg recreated from stapled app: $DMG"
fi
