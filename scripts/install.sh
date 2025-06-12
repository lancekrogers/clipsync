#!/usr/bin/env bash

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
REPO_OWNER="yourusername"
REPO_NAME="clipsync"
BINARY_NAME="clipsync"
VERSION="${VERSION:-latest}"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
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
    
    local platform_suffix=""
    case "$os-$arch" in
        "linux-x86_64")     platform_suffix="x86_64-unknown-linux-gnu";;
        "linux-aarch64")    platform_suffix="aarch64-unknown-linux-gnu";;
        "macos-x86_64")     platform_suffix="x86_64-apple-darwin";;
        "macos-aarch64")    platform_suffix="aarch64-apple-darwin";;
        *)
            log_error "Unsupported platform: $os-$arch"
            exit 1
            ;;
    esac
    
    if [ "$version" = "latest" ]; then
        echo "https://github.com/$REPO_OWNER/$REPO_NAME/releases/latest/download/${BINARY_NAME}-${platform_suffix}.tar.gz"
    else
        echo "https://github.com/$REPO_OWNER/$REPO_NAME/releases/download/v${version}/${BINARY_NAME}-${version}-${platform_suffix}.tar.gz"
    fi
}

download_and_install() {
    local url=$1
    local temp_dir=$(mktemp -d)
    
    log_info "Downloading ClipSync from $url..."
    if ! curl -sL "$url" -o "$temp_dir/clipsync.tar.gz"; then
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
    
    log_info "Installing binary to $INSTALL_DIR..."
    if [ -w "$INSTALL_DIR" ]; then
        cp "$temp_dir/$BINARY_NAME" "$INSTALL_DIR/"
        chmod 755 "$INSTALL_DIR/$BINARY_NAME"
    else
        log_warn "Need sudo permissions to install to $INSTALL_DIR"
        sudo cp "$temp_dir/$BINARY_NAME" "$INSTALL_DIR/"
        sudo chmod 755 "$INSTALL_DIR/$BINARY_NAME"
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
    local plist_path="$HOME/Library/LaunchAgents/com.clipsync.plist"
    local plist_dir=$(dirname "$plist_path")
    
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
        <string>$INSTALL_DIR/$BINARY_NAME</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$DATA_DIR/clipsync.log</string>
    <key>StandardErrorPath</key>
    <string>$DATA_DIR/clipsync.error.log</string>
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
    launchctl load "$plist_path" 2>/dev/null || true
    
    log_info "ClipSync service installed for macOS!"
    log_info "To manage the service:"
    log_info "  Start:   launchctl load $plist_path"
    log_info "  Stop:    launchctl unload $plist_path"
    log_info "  Status:  launchctl list | grep clipsync"
}

install_service_linux() {
    local service_path="$HOME/.config/systemd/user/clipsync.service"
    local service_dir=$(dirname "$service_path")
    
    log_info "Installing systemd service..."
    mkdir -p "$service_dir"
    
    cat > "$service_path" <<EOF
[Unit]
Description=ClipSync - Cross-platform clipboard synchronization
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$INSTALL_DIR/$BINARY_NAME
Restart=always
RestartSec=10
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=default.target
EOF
    
    log_info "Reloading systemd and enabling service..."
    systemctl --user daemon-reload
    systemctl --user enable clipsync.service
    
    log_info "ClipSync service installed for Linux!"
    log_info "To manage the service:"
    log_info "  Start:   systemctl --user start clipsync"
    log_info "  Stop:    systemctl --user stop clipsync"
    log_info "  Status:  systemctl --user status clipsync"
    log_info "  Logs:    journalctl --user -u clipsync -f"
}

main() {
    echo "╔══════════════════════════════════════╗"
    echo "║       ClipSync Installer v1.0        ║"
    echo "╚══════════════════════════════════════╝"
    echo
    
    # Check dependencies
    check_dependencies
    
    # Detect platform
    local os=$(detect_os)
    local arch=$(detect_arch)
    
    if [ "$os" = "unknown" ] || [ "$arch" = "unknown" ]; then
        log_error "Unsupported platform detected"
        exit 1
    fi
    
    log_info "Detected platform: $os-$arch"
    
    # Get download URL
    local download_url=$(get_download_url "$os" "$arch" "$VERSION")
    
    # Download and install
    download_and_install "$download_url"
    
    # Setup directories
    setup_directories
    
    # Install service
    case "$os" in
        "macos")
            install_service_macos
            ;;
        "linux")
            install_service_linux
            ;;
    esac
    
    echo
    log_info "Installation complete! 🎉"
    log_info "ClipSync is now installed and running as a background service."
    echo
    
    # Verify installation
    if command -v "$BINARY_NAME" &> /dev/null; then
        log_info "ClipSync version: $($BINARY_NAME --version 2>/dev/null || echo 'unknown')"
    fi
}

# Run main function
main "$@"