#!/usr/bin/env bash
# Isolated install helper tests (env-driven; do not override HOME in callers).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_SCRIPTS="$(cd "$SCRIPT_DIR/.." && pwd)"

# shellcheck source=../lib/linux_user_service.sh
source "$REPO_SCRIPTS/lib/linux_user_service.sh"

case "${CLIPSYNC_TEST_CASE:?}" in
    install_service)
        clipsync_install_linux_user_service "${CLIPSYNC_INSTALL_BIN:?}" 0
        ;;
    missing_binary)
        if clipsync_install_linux_user_service "${CLIPSYNC_INSTALL_BIN:?}" 0; then
            exit 9
        fi
        ;;
    reload_fail)
        if clipsync_install_linux_user_service "${CLIPSYNC_INSTALL_BIN:?}" 0; then
            exit 9
        fi
        ;;
    enable_fail)
        if clipsync_install_linux_user_service "${CLIPSYNC_INSTALL_BIN:?}" 0; then
            exit 9
        fi
        ;;
    start_fail)
        set +e
        clipsync_install_linux_user_service "${CLIPSYNC_INSTALL_BIN:?}" 1
        rc=$?
        set -e
        if [[ "$rc" -ne 2 ]]; then
            echo "expected exit 2, got $rc" >&2
            exit 1
        fi
        ;;
    write_unit)
        clipsync_write_user_unit "${CLIPSYNC_INSTALL_BIN:?}"
        ;;
    ensure_config_fresh)
        clipsync_ensure_default_config "${CLIPSYNC_INSTALL_BIN:?}"
        ;;
    ensure_config_reinstall)
        clipsync_ensure_default_config "${CLIPSYNC_INSTALL_BIN:?}"
        ;;
    uninstall_user_unit)
        UNIT_PATH="$(clipsync_user_unit_path)"
        if [[ -f "$UNIT_PATH" ]]; then
            clipsync_systemctl disable clipsync.service 2>/dev/null || true
            clipsync_systemctl stop clipsync.service 2>/dev/null || true
            rm -f "$UNIT_PATH"
            clipsync_reload_user_systemd 2>/dev/null || true
        fi
        ;;
    *)
        echo "unknown CLIPSYNC_TEST_CASE: ${CLIPSYNC_TEST_CASE}" >&2
        exit 1
        ;;
esac
