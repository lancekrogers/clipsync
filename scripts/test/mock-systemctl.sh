#!/usr/bin/env bash
# Mock systemctl for isolated install tests. Logs invocations; does not touch the host.

set -euo pipefail

LOG_FILE="${CLIPSYNC_MOCK_SYSTEMCTL_LOG:-/dev/null}"

log() {
    printf '%s\n' "$*" >>"$LOG_FILE"
}

if [[ "${1:-}" != "--user" ]]; then
    log "reject: expected systemctl --user, got: $*"
    echo "mock-systemctl: only --user scope is supported" >&2
    exit 1
fi

shift
log "scope: --user"

cmd="${1:-}"
case "$cmd" in
    daemon-reload)
        if [[ "${CLIPSYNC_MOCK_SYSTEMCTL_RELOAD_FAIL:-0}" == "1" ]]; then
            log "daemon-reload (failed)"
            exit 1
        fi
        log "daemon-reload"
        ;;
    enable)
        if [[ "${CLIPSYNC_MOCK_SYSTEMCTL_ENABLE_FAIL:-0}" == "1" ]]; then
            log "enable ${2:-} (failed)"
            exit 1
        fi
        log "enable ${2:-}"
        ;;
    start)
        if [[ "${CLIPSYNC_MOCK_SYSTEMCTL_START_FAIL:-0}" == "1" ]]; then
            log "start ${2:-} (failed)"
            exit 1
        fi
        log "start ${2:-}"
        ;;
    stop)
        log "stop ${2:-}"
        ;;
    disable)
        log "disable ${2:-}"
        ;;
    *)
        log "unknown: $*"
        exit 1
        ;;
esac

exit 0
