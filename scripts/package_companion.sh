#!/usr/bin/env bash
# Entry point for ClipSync companion local-dev packaging.
# Does not edit the root justfile; import just/companion.just from the root when ready.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PKG="$SCRIPT_DIR/package"

usage() {
  cat <<EOF
Usage: $0 <command>

Commands:
  binaries     Build CLI (--features tui) + GUI (no integration-tests/ui-test)
  macos        Assemble ClipSync.app + UNSIGNED development DMG (macOS host)
  linux        Assemble user-local archive + desktop entry + icon
  all          Package for the current host OS
  verify       Static layout/manifest checks (pass-through args)
  test         Isolated package tests (tempdirs; no host service changes)
  sign-macos   Developer ID + notarize (hard-gated on credentials; usually blocked)

Environment:
  CLIPSYNC_RUSTUP_TOOLCHAIN   default 1.95.0; set empty to use active toolchain
  CLIPSYNC_PACKAGE_DIST       output root (default: <repo>/dist/companion)
  CLIPSYNC_PACKAGE_FORCE=1    replace existing package outputs
  CLIPSYNC_PACKAGE_SKIP_BUILD=1   reuse CLIPSYNC_PACKAGE_{CLI,GUI}_BIN
  CLIPSYNC_PACKAGE_FIXTURE_MODE=1 allow non-Linux host linux assemble + script bins
  CLIPSYNC_PACKAGE_FIXTURE_PREFIX fixture install root (tests; avoids real HOME)
  CLIPSYNC_PACKAGE_MOCK_BIN   stub PATH for sign-macos flow tests
  CARGO_TARGET_DIR            optional isolated cargo target (coordinate with other workers)

Artifacts are labeled UNSIGNED-DEVELOPMENT unless sign-macos succeeds with real credentials.
Fixture packages must not be claimed as release/ELF/Mach-O proof.
EOF
}

cmd="${1:-}"
shift || true

case "$cmd" in
  binaries) exec bash "$PKG/build-binaries.sh" "$@" ;;
  macos) exec bash "$PKG/build-macos-companion.sh" "$@" ;;
  linux) exec bash "$PKG/build-linux-companion.sh" "$@" ;;
  all) exec bash "$PKG/build-all-companion.sh" "$@" ;;
  verify) exec bash "$PKG/verify.sh" "$@" ;;
  test) exec bash "$PKG/test_package.sh" "$@" ;;
  sign-macos) exec bash "$PKG/sign-macos.sh" "$@" ;;
  -h|--help|help|"") usage; exit 0 ;;
  *)
    echo "unknown command: $cmd" >&2
    usage >&2
    exit 2
    ;;
esac
