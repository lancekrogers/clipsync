#!/usr/bin/env bash
# Shared helpers for per-user systemd installation (no root service).

set -euo pipefail

clipsync_user_unit_dir() {
    if [[ -n "${CLIPSYNC_SYSTEMD_USER_DIR:-}" ]]; then
        printf '%s\n' "$CLIPSYNC_SYSTEMD_USER_DIR"
        return
    fi
    printf '%s\n' "${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user"
}

clipsync_user_unit_path() {
    printf '%s/clipsync.service\n' "$(clipsync_user_unit_dir)"
}

clipsync_install_bin_dir() {
    if [[ -n "${CLIPSYNC_INSTALL_DIR:-}" ]]; then
        printf '%s\n' "$CLIPSYNC_INSTALL_DIR"
        return
    fi
    printf '%s\n' "${HOME}/.local/bin"
}

clipsync_config_dir() {
    printf '%s/clipsync\n' "${XDG_CONFIG_HOME:-${HOME}/.config}"
}

clipsync_ensure_default_config() {
    local binary="$1"
    local config_dir config_file
    config_dir="$(clipsync_config_dir)"
    config_file="$config_dir/config.toml"
    mkdir -p "$config_dir"
    if [[ -f "$config_file" ]]; then
        return 0
    fi
    CLIPSYNC_CONFIG="$config_file" "$binary" config init
}

clipsync_unit_renderer() {
    if [[ -n "${CLIPSYNC_UNIT_RENDERER:-}" ]]; then
        printf '%s\n' "$CLIPSYNC_UNIT_RENDERER"
        return
    fi
    printf '%s\n' "${CLIPSYNC_INSTALL_BIN:-}"
}

clipsync_write_user_unit() {
    local binary="$1"
    local unit_path renderer temporary_unit

    if [[ -z "$binary" ]]; then
        echo "clipsync_write_user_unit: binary path is required" >&2
        return 1
    fi

    unit_path="$(clipsync_user_unit_path)"
    mkdir -p "$(dirname "$unit_path")"

    renderer="$(clipsync_unit_renderer)"
    if [[ -z "$renderer" ]]; then
        renderer="$binary"
    fi
    if [[ ! -x "$renderer" ]]; then
        echo "clipsync_write_user_unit: renderer is not executable: $renderer" >&2
        return 1
    fi

    temporary_unit=$(mktemp "${unit_path}.XXXXXX") || return 1
    if ! "$renderer" print-user-unit --binary "$binary" >"$temporary_unit"; then
        rm -f "$temporary_unit"
        return 1
    fi
    chmod 644 "$temporary_unit" || { rm -f "$temporary_unit"; return 1; }
    mv -f "$temporary_unit" "$unit_path" || { rm -f "$temporary_unit"; return 1; }
}

clipsync_systemctl() {
    if [[ -n "${CLIPSYNC_SYSTEMCTL:-}" ]]; then
        "$CLIPSYNC_SYSTEMCTL" --user "$@"
        return
    fi
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user "$@"
        return
    fi
    echo "systemctl --user is not available" >&2
    return 1
}

clipsync_reload_user_systemd() {
    clipsync_systemctl daemon-reload
}

clipsync_enable_user_service() {
    clipsync_systemctl enable clipsync.service
}

clipsync_try_start_user_service() {
    clipsync_systemctl start clipsync.service
}

clipsync_install_linux_user_service() {
    local binary="$1"
    local start_service="${2:-0}"

    if [[ ! -x "$binary" ]]; then
        echo "ClipSync binary is missing or not executable: $binary" >&2
        return 1
    fi

    clipsync_write_user_unit "$binary" || return 1
    clipsync_reload_user_systemd || return 1
    clipsync_enable_user_service || return 1

    if [[ "$start_service" == "1" ]]; then
        if ! clipsync_try_start_user_service; then
            echo "Service unit was installed and enabled, but start failed." >&2
            echo "Check: systemctl --user status clipsync" >&2
            return 2
        fi
    fi

    return 0
}

# Replace an executable atomically so reinstall also works while the old binary is running.
clipsync_install_binary() {
    local source_binary="$1" destination="$2" temporary_binary
    [[ -f "$source_binary" ]] || { echo "Missing binary: $source_binary" >&2; return 1; }
    mkdir -p "$(dirname "$destination")" || return 1
    temporary_binary=$(mktemp "${destination}.XXXXXX") || return 1
    cp "$source_binary" "$temporary_binary" && chmod 755 "$temporary_binary" && mv -f "$temporary_binary" "$destination" && return 0
    rm -f "$temporary_binary"
    return 1
}
