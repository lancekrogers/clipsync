#!/usr/bin/env bash
# Package companion artifacts for the current host platform only.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

OS="$(uname -s)"
clipsync_package_log "ClipSync companion packager (local-dev / unsigned by default)"
clipsync_package_log "host: $OS $(clipsync_package_arch) version=$(clipsync_package_version)"

case "$OS" in
  Darwin)
    bash "$SCRIPT_DIR/build-macos-companion.sh"
    ;;
  Linux)
    bash "$SCRIPT_DIR/build-linux-companion.sh"
    ;;
  *)
    clipsync_package_die "unsupported host OS: $OS"
    ;;
esac

clipsync_package_log "done. Artifacts under $(clipsync_package_dist_root)"
clipsync_package_log "These are UNSIGNED development artifacts — not public-release-ready."
