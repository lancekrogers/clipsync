#!/usr/bin/env bash
# Static manifest / package layout verification. No host service mutations.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

USAGE='Usage: verify.sh [--macos APP_DIR] [--linux BUNDLE_DIR] [--manifest FILE] [--acceptance FILE]'

MACOS_APP=""
LINUX_BUNDLE=""
MANIFEST=""
ACCEPTANCE=""
FAILURES=0

fail() {
  printf 'VERIFY FAIL: %s\n' "$*" >&2
  FAILURES=$((FAILURES + 1))
}

ok() {
  printf 'VERIFY OK: %s\n' "$*"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --macos) MACOS_APP="${2:-}"; shift 2 ;;
    --linux) LINUX_BUNDLE="${2:-}"; shift 2 ;;
    --manifest) MANIFEST="${2:-}"; shift 2 ;;
    --acceptance) ACCEPTANCE="${2:-}"; shift 2 ;;
    -h|--help) echo "$USAGE"; exit 0 ;;
    *) clipsync_package_die "unknown arg: $1" ;;
  esac
done

if [[ -z "$MACOS_APP" && -z "$LINUX_BUNDLE" && -z "$MANIFEST" && -z "$ACCEPTANCE" ]]; then
  clipsync_package_die "$USAGE"
fi

verify_macos() {
  local app="$1"
  [[ -d "$app" ]] || { fail "macOS app missing: $app"; return; }
  local exe="$app/Contents/MacOS/clipsync-desktop"
  local cli="$app/Contents/MacOS/clipsync"
  local plist="$app/Contents/Info.plist"
  [[ -f "$exe" ]] || fail "missing CFBundleExecutable binary: $exe"
  [[ -f "$cli" ]] || fail "missing sibling CLI: $cli"
  [[ -x "$exe" ]] || fail "GUI not executable: $exe"
  [[ -x "$cli" ]] || fail "CLI not executable: $cli"
  [[ -f "$plist" ]] || fail "missing Info.plist"
  grep -q 'clipsync-desktop' "$plist" || fail "Info.plist CFBundleExecutable not clipsync-desktop"
  grep -q 'NSHighResolutionCapable' "$plist" || fail "Info.plist missing NSHighResolutionCapable"
  if grep -qi 'San Francisco\|SF Pro\|SF-Pro' "$app/Contents/Resources"/* 2>/dev/null; then
    fail "bundle appears to include Apple SF font payloads"
  else
    ok "no bundled SF font payloads detected"
  fi
  if [[ -f "$app/Contents/Resources/AppIcon.icns" ]]; then
    grep -q 'CFBundleIconFile' "$plist" || fail "AppIcon.icns present but CFBundleIconFile missing"
    ok "AppIcon.icns present"
  else
    ok "no AppIcon.icns (optional when converters unavailable)"
  fi
  if [[ -f "$app/Contents/Resources/PROVENANCE.json" ]]; then
    if grep -q '"fixture_mode": true' "$app/Contents/Resources/PROVENANCE.json"; then
      ok "provenance records fixture_mode=true (features not verified)"
      grep -q '"verified": false' "$app/Contents/Resources/PROVENANCE.json" \
        || fail "fixture provenance must set features.verified=false"
    else
      grep -q 'UNSIGNED-DEVELOPMENT' "$app/Contents/Resources/PROVENANCE.json" \
        || fail "PROVENANCE.json missing UNSIGNED-DEVELOPMENT label"
      grep -q '"notarized": false' "$app/Contents/Resources/PROVENANCE.json" \
        || fail "unsigned PROVENANCE.json must declare notarized=false"
      ok "provenance labels unsigned development"
    fi
  else
    fail "missing PROVENANCE.json"
  fi

  # Host format check when not fixture scripts.
  local kind
  kind="$(clipsync_package_binary_kind "$exe")"
  if [[ "$kind" == "macho" ]]; then
    ok "GUI is Mach-O"
  elif [[ "$kind" == "script" ]]; then
    ok "GUI is fixture script (not release proof)"
  else
    fail "unexpected GUI binary kind: $kind"
  fi
  ok "macOS layout $app"
}

verify_linux() {
  local bundle="$1"
  [[ -d "$bundle" ]] || { fail "linux bundle missing: $bundle"; return; }
  [[ -f "$bundle/bin/clipsync" ]] || fail "missing bin/clipsync"
  [[ -f "$bundle/bin/clipsync-desktop" ]] || fail "missing bin/clipsync-desktop"
  [[ -x "$bundle/bin/clipsync" ]] || fail "cli not executable"
  [[ -x "$bundle/bin/clipsync-desktop" ]] || fail "gui not executable"
  [[ -f "$bundle/share/icons/hicolor/scalable/apps/clipsync.svg" ]] || fail "missing SVG icon"
  [[ -f "$bundle/share/applications/clipsync.desktop.in" ]] || fail "missing desktop template"
  [[ -x "$bundle/install-user.sh" ]] || fail "missing install-user.sh"
  [[ -f "$bundle/share/doc/clipsync/UNINSTALL.md" ]] || fail "missing UNINSTALL.md"
  [[ -f "$bundle/ClipSync-Install.desktop" ]] || fail "missing ClipSync-Install.desktop"
  [[ -f "$bundle/share/doc/clipsync/LIMITATIONS.md" ]] || fail "missing LIMITATIONS.md"

  # Relative from-bundle launcher must NOT exist (CWD-relative Exec is broken).
  if [[ -f "$bundle/ClipSync-from-bundle.desktop" ]]; then
    fail "ClipSync-from-bundle.desktop must not ship (relative Exec breaks under file managers)"
  else
    ok "no relative-Exec from-bundle launcher"
  fi

  local desktop
  for desktop in "$bundle/ClipSync-Install.desktop"; do
    if grep -E 'Exec=.*\$\(|Exec=.*`' "$desktop" >/dev/null; then
      fail "shell injection pattern (\$( or backtick) in $desktop"
    else
      ok "Exec= has no \$( / backtick injection patterns in $(basename "$desktop")"
    fi
    if grep -q '^Terminal=false' "$desktop"; then
      ok "install desktop is Terminal=false (terminal-free)"
    else
      fail "ClipSync-Install.desktop must set Terminal=false for D05 terminal-free install"
    fi
    if grep -qE '^Exec=\./' "$desktop"; then
      fail "relative Exec in $desktop (CWD-relative; breaks under file managers)"
    fi
    # Portable entrypoint: %k locates the .desktop; must not bake absolute install-user.sh.
    if grep -qE '%k' "$desktop"; then
      ok "install desktop uses %k for self-location"
    else
      fail "ClipSync-Install.desktop must use %k so extract-anywhere activation works"
    fi
    if grep -qE -- '-c' "$desktop"; then
      ok "install desktop uses /bin/sh -c locator (not a baked script path argv)"
    else
      fail "ClipSync-Install.desktop must use sh -c + %k (not a direct absolute script path)"
    fi
    # Default packaged form must stay portable (%k). Absolute form is opt-in via
    # write-install-launcher.sh --absolute after extract (for gio launch CLI).
    if grep -qE '^Exec="?/.*/install-user\.sh"' "$desktop" \
      || grep -qE '^Exec=/.*/install-user\.sh([[:space:]]|$)' "$desktop"; then
      fail "packaged ClipSync-Install.desktop must be portable (%k), not absolute install-user.sh Exec"
    fi
    if grep -qE '^Path=' "$desktop"; then
      fail "ClipSync-Install.desktop must not set Path= (package-time absolute Path breaks after move)"
    fi
  done

  if [[ -f "$bundle/share/doc/clipsync/PROVENANCE.json" ]]; then
    if grep -q '"fixture_mode": true' "$bundle/share/doc/clipsync/PROVENANCE.json"; then
      ok "linux provenance fixture_mode=true"
    else
      grep -q 'UNSIGNED-DEVELOPMENT' "$bundle/share/doc/clipsync/PROVENANCE.json" \
        || fail "PROVENANCE missing UNSIGNED label"
      ok "linux provenance unsigned"
    fi
  else
    fail "missing linux PROVENANCE.json"
  fi

  local kind
  kind="$(clipsync_package_binary_kind "$bundle/bin/clipsync")"
  case "$kind" in
    elf) ok "CLI is ELF" ;;
    script) ok "CLI is fixture script (not Linux release proof)" ;;
    macho) fail "CLI is Mach-O inside a Linux archive (host mixup)" ;;
    *) fail "unexpected CLI kind: $kind" ;;
  esac
  ok "linux layout $bundle"
}

verify_manifest() {
  local file="$1"
  [[ -f "$file" ]] || { fail "manifest missing: $file"; return; }
  grep -q '"public_release_ready": false' "$file" || fail "manifest must set public_release_ready=false"
  if grep -q '"fixture_mode": true' "$file"; then
    ok "manifest fixture_mode recorded"
    grep -q '"features_verified": false' "$file" || fail "fixture manifest must set features_verified=false"
  else
    grep -q 'UNSIGNED' "$file" || fail "manifest must label UNSIGNED"
  fi
  ok "manifest $file"
}

verify_acceptance() {
  local file="$1"
  [[ -f "$file" ]] || { fail "acceptance missing: $file"; return; }
  if grep -q '"status": "accepted"' "$file"; then
    grep -q '"notarized": true' "$file" || fail "accepted acceptance must set notarized=true"
    grep -q '"public_release_ready": true' "$file" || fail "accepted acceptance must set public_release_ready=true"
    ok "acceptance accepted"
  elif grep -q '"status": "failed"' "$file"; then
    grep -q '"public_release_ready": false' "$file" || fail "failed acceptance must keep public_release_ready=false"
    grep -q '"notarized": false' "$file" || fail "failed acceptance must keep notarized=false"
    ok "acceptance failed (honest non-release)"
  else
    fail "acceptance status not recognized"
  fi
}

[[ -n "$MACOS_APP" ]] && verify_macos "$MACOS_APP"
[[ -n "$LINUX_BUNDLE" ]] && verify_linux "$LINUX_BUNDLE"
[[ -n "$MANIFEST" ]] && verify_manifest "$MANIFEST"
[[ -n "$ACCEPTANCE" ]] && verify_acceptance "$ACCEPTANCE"

if [[ "$FAILURES" -gt 0 ]]; then
  printf 'verify: %s failure(s)\n' "$FAILURES" >&2
  exit 1
fi
printf 'verify: all checks passed\n'
