#!/usr/bin/env bash
# Build matching companion CLI (tui) + GUI without integration-tests / ui-test.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

ROOT="$(clipsync_package_project_root)"
TARGET_DIR="$(clipsync_package_target_dir)"
VERSION="$(clipsync_package_version)"

clipsync_package_log "building companion binaries v${VERSION}"
clipsync_package_log "toolchain pin: ${CLIPSYNC_RUSTUP_TOOLCHAIN-1.95.0} (does not change rustup default)"
clipsync_package_log "CARGO_TARGET_DIR=$TARGET_DIR"

cd "$ROOT"
export CARGO_TARGET_DIR="$TARGET_DIR"

# CLI/daemon + TUI entrypoint. Explicitly omit integration-tests. --locked for reproducible builds.
clipsync_package_cargo build --locked --release -p clipsync --features tui --bin clipsync

# Desktop GUI. Explicitly omit ui-test (no eframe inspection server).
# Separate invocation so GUI root deps without tui do not rebuild away the CLI binary.
clipsync_package_cargo build --locked --release -p clipsync-desktop --bin clipsync-desktop

CLI_BIN="$TARGET_DIR/release/clipsync"
GUI_BIN="$TARGET_DIR/release/clipsync-desktop"

[[ -x "$CLI_BIN" ]] || clipsync_package_die "missing CLI binary: $CLI_BIN"
[[ -x "$GUI_BIN" ]] || clipsync_package_die "missing GUI binary: $GUI_BIN"

clipsync_package_log "built: $CLI_BIN"
clipsync_package_log "built: $GUI_BIN"
printf '%s\n' "$CLI_BIN"
printf '%s\n' "$GUI_BIN"
