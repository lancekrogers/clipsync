#!/usr/bin/env bash

set -euo pipefail

if [[ -n "${BASH_SOURCE[0]:-}" && "${BASH_SOURCE[0]}" != "bash" && -f "${BASH_SOURCE[0]}" ]]; then
    INSTALL_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
else
    INSTALL_SCRIPT_DIR=""
fi

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
REPO_OWNER="lancekrogers"
REPO_NAME="clipsync"
BINARY_NAME="clipsync"
VERSION="${VERSION:-latest}"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/clipsync"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/clipsync"

# Helper functions
log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1" >&2
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

detect_os() {
    case "$(uname -s)" in
        Linux*)     echo "linux";;
        Darwin*)    echo "macos";;
        *)          echo "unknown";;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64)     echo "x86_64";;
        aarch64)    echo "aarch64";;
        arm64)      echo "aarch64";;
        *)          echo "unknown";;
    esac
}

resolve_install_dir() {
    local os="$1"
    if [[ -n "${INSTALL_DIR:-}" ]]; then
        printf '%s\n' "$INSTALL_DIR"
        return
    fi
    if [[ "$os" == "linux" ]]; then
        printf '%s\n' "${HOME}/.local/bin"
    else
        printf '%s\n' "/usr/local/bin"
    fi
}

check_dependencies() {
    local deps=("curl" "tar")
    for dep in "${deps[@]}"; do
        if ! command -v "$dep" &> /dev/null; then
            log_error "$dep is required but not installed"
            exit 1
        fi
    done
}

get_download_url() {
    local os=$1
    local arch=$2
    local version=$3
    
    case "$os-$arch" in
        linux-x86_64|linux-aarch64|macos-x86_64|macos-aarch64) ;;
        *) log_error "Unsupported platform: $os-$arch"; return 1 ;;
    esac
    local asset="${BINARY_NAME}-${os}-${arch}.tar.gz"
    if [[ "$version" == "latest" ]]; then
        printf 'https://github.com/%s/%s/releases/latest/download/%s\n' "$REPO_OWNER" "$REPO_NAME" "$asset"
    else
        printf 'https://github.com/%s/%s/releases/download/v%s/%s\n' "$REPO_OWNER" "$REPO_NAME" "${version#v}" "$asset"
    fi
}

download_and_install() {
    local url=$1
    local install_dir=$2
    local temp_dir
    temp_dir=$(mktemp -d)
    
    log_info "Downloading ClipSync from $url..."
    if ! curl -fsSL "$url" -o "$temp_dir/clipsync.tar.gz"; then
        log_error "Failed to download ClipSync"
        rm -rf "$temp_dir"
        exit 1
    fi
    
    log_info "Extracting archive..."
    if ! tar -xzf "$temp_dir/clipsync.tar.gz" -C "$temp_dir"; then
        log_error "Failed to extract archive"
        rm -rf "$temp_dir"
        exit 1
    fi
    
    mkdir -p "$install_dir"
    log_info "Installing binary to $install_dir..."
    if [ -w "$install_dir" ]; then
        cp "$temp_dir/$BINARY_NAME" "$install_dir/"
        chmod 755 "$install_dir/$BINARY_NAME"
    else
        log_warn "Need elevated permissions to install to $install_dir"
        sudo cp "$temp_dir/$BINARY_NAME" "$install_dir/"
        sudo chmod 755 "$install_dir/$BINARY_NAME"
    fi
    
    rm -rf "$temp_dir"
    log_info "ClipSync binary installed successfully!"
}

setup_directories() {
    log_info "Creating configuration directories..."
    mkdir -p "$CONFIG_DIR"
    mkdir -p "$DATA_DIR"
    chmod 700 "$CONFIG_DIR"
    chmod 700 "$DATA_DIR"
}

install_service_macos() {
    local install_dir=$1
    local binary="$install_dir/$BINARY_NAME"
    local plist_path="$HOME/Library/LaunchAgents/com.clipsync.plist"
    local plist_dir
    plist_dir=$(dirname "$plist_path")
    
    log_info "Installing launchd service..."
    mkdir -p "$plist_dir"
    
    cat > "$plist_path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.clipsync</string>
    <key>ProgramArguments</key>
    <array>
        <string>$binary</string>
        <string>start</string>
        <string>--foreground</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$CONFIG_DIR/clipsync.log</string>
    <key>StandardErrorPath</key>
    <string>$CONFIG_DIR/clipsync.error.log</string>
    <key>WorkingDirectory</key>
    <string>$DATA_DIR</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>$HOME</string>
    </dict>
</dict>
</plist>
EOF
    
    log_info "Loading launchd service..."
    if launchctl bootstrap "gui/$(id -u)" "$plist_path" 2>/dev/null || launchctl load "$plist_path" 2>/dev/null; then
        log_info "LaunchAgent loaded"
    else
        log_warn "LaunchAgent was written but could not be loaded automatically"
    fi
    
    log_info "ClipSync service installed for macOS!"
    log_info "To manage the service:"
    log_info "  Start:   launchctl load $plist_path"
    log_info "  Stop:    launchctl unload $plist_path"
    log_info "  Status:  launchctl list | grep clipsync"
}

install_service_linux() {
    local install_dir=$1
    local binary="$install_dir/$BINARY_NAME"

    log_info "Installing per-user systemd service (no root clipboard daemon)..."
    if [[ -n "$INSTALL_SCRIPT_DIR" && -f "$INSTALL_SCRIPT_DIR/lib/linux_user_service.sh" ]]; then
        # shellcheck source=lib/linux_user_service.sh
        source "$INSTALL_SCRIPT_DIR/lib/linux_user_service.sh"
        clipsync_install_linux_user_service "$binary" 0 || return 1
    else
        local unit_path="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/clipsync.service"
        mkdir -p "$(dirname "$unit_path")"
        local temporary_unit
        temporary_unit=$(mktemp "${unit_path}.XXXXXX") || return 1
        if ! "$binary" print-user-unit --binary "$binary" >"$temporary_unit"; then
            rm -f "$temporary_unit"
            log_error "The downloaded binary must support print-user-unit; use a release built with this installer."
            return 1
        fi
        chmod 644 "$temporary_unit" || { rm -f "$temporary_unit"; return 1; }
        mv -f "$temporary_unit" "$unit_path" || { rm -f "$temporary_unit"; return 1; }
        systemctl --user daemon-reload || return 1
        systemctl --user enable clipsync.service || return 1
    fi

    log_info "ClipSync user service installed for Linux!"
    log_info "To manage the service:"
    log_info "  Start:   systemctl --user start clipsync"
    log_info "  Stop:    systemctl --user stop clipsync"
    log_info "  Status:  systemctl --user status clipsync"
    log_info "  Logs:    journalctl --user -u clipsync -f"
    log_info "If an older system-wide clipsync.service exists, review it before disabling:"
    log_info "  sudo systemctl status clipsync"
}

main() {
    echo "╔══════════════════════════════════════╗"
    echo "║       ClipSync Installer v1.0        ║"
    echo "╚══════════════════════════════════════╝"
    echo
    
    check_dependencies
    
    local os
    os=$(detect_os)
    local arch
    arch=$(detect_arch)
    
    if [ "$os" = "unknown" ] || [ "$arch" = "unknown" ]; then
        log_error "Unsupported platform detected"
        exit 1
    fi
    
    local install_dir
    install_dir=$(resolve_install_dir "$os")
    log_info "Detected platform: $os-$arch"
    log_info "Install directory: $install_dir"
    
    local download_url
    download_url=$(get_download_url "$os" "$arch" "$VERSION")
    
    download_and_install "$download_url" "$install_dir"
    setup_directories
    
    local service_ok=1
    case "$os" in
        "macos")
            install_service_macos "$install_dir" || service_ok=0
            ;;
        "linux")
            install_service_linux "$install_dir" || service_ok=0
            ;;
    esac
    
    echo
    if [[ "$service_ok" == "1" ]]; then
        log_info "Installation complete."
        log_info "The desktop service unit is installed; start it with your platform's service manager if needed."
    else
        log_warn "Binary installation finished, but the desktop service step failed."
        log_warn "Fix the reported issues and rerun the service install step."
        exit 1
    fi
    echo
    
    if [[ ":$PATH:" != *":$install_dir:"* ]] && [[ -x "$install_dir/$BINARY_NAME" ]]; then
        log_info "Add $install_dir to your PATH if needed."
    fi
    if command -v "$BINARY_NAME" &> /dev/null; then
        log_info "ClipSync version: $($BINARY_NAME --version 2>/dev/null || echo 'unknown')"
    elif [[ -x "$install_dir/$BINARY_NAME" ]]; then
        log_info "ClipSync version: $("$install_dir/$BINARY_NAME" --version 2>/dev/null || echo 'unknown')"
    fi
}

main "$@"
