#!/usr/bin/env bash
# User-specific installation script (no sudo required)

set -euo pipefail

echo "ClipSync User Installation (No sudo required)"
echo "============================================"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [[ "$OSTYPE" == "darwin"* ]]; then
    OS="macos"
elif [[ "$OSTYPE" == "linux-gnu"* ]] || [[ "$(uname -s)" == "Linux" ]]; then
    OS="linux"
else
    echo "Unsupported OS for user installation: $OSTYPE"
    exit 1
fi

INSTALL_DIR="${CLIPSYNC_INSTALL_DIR:-${HOME}/.local/bin}"
BINARY="${INSTALL_DIR}/clipsync"

# shellcheck source=lib/linux_user_service.sh
source "$SCRIPT_DIR/lib/linux_user_service.sh"
CONFIG_DIR="$(clipsync_config_dir)"

mkdir -p "$INSTALL_DIR"
mkdir -p "$CONFIG_DIR"

if [[ "$OS" == "macos" ]]; then
    mkdir -p "${HOME}/Library/LaunchAgents"
fi

# Build if not already built
RELEASE_BIN="${CLIPSYNC_BINARY_SOURCE:-${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}/release/clipsync}"
if [[ ! -f "$RELEASE_BIN" ]]; then
    if [[ -n "${CLIPSYNC_BINARY_SOURCE:-}" ]]; then
        echo "Configured binary source is missing: $RELEASE_BIN" >&2
        exit 1
    fi
    echo "Building ClipSync..."
    (cd "$PROJECT_ROOT" && cargo build --release)
fi

echo "Installing binary to ${INSTALL_DIR}/..."
clipsync_install_binary "$RELEASE_BIN" "$BINARY"

if [[ "$OS" == "macos" ]]; then
    echo "Creating LaunchAgent..."
    cat > "${HOME}/Library/LaunchAgents/com.clipsync.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.clipsync</string>
    <key>ProgramArguments</key>
    <array>
        <string>${BINARY}</string>
        <string>--config</string>
        <string>${CONFIG_DIR}/config.toml</string>
        <string>start</string>
        <string>--foreground</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>${CONFIG_DIR}/clipsync.out</string>
    <key>StandardErrorPath</key>
    <string>${CONFIG_DIR}/clipsync.err</string>
    <key>WorkingDirectory</key>
    <string>${HOME}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>${INSTALL_DIR}:/usr/local/bin:/usr/bin:/bin</string>
    </dict>
</dict>
</plist>
EOF

    clipsync_ensure_default_config "$BINARY"

    echo ""
    echo "Loading LaunchAgent..."
    if launchctl bootstrap "gui/$(id -u)" "${HOME}/Library/LaunchAgents/com.clipsync.plist" 2>/dev/null \
        || launchctl load "${HOME}/Library/LaunchAgents/com.clipsync.plist" 2>/dev/null; then
        echo "LaunchAgent loaded"
    else
        echo "LaunchAgent written but could not be loaded automatically; use launchctl load manually"
        exit 2
    fi
else
    clipsync_ensure_default_config "$BINARY"

    echo "Installing per-user systemd unit..."
    clipsync_install_linux_user_service "$BINARY" 0
    echo "User service enabled. Start after graphical login with:"
    echo "  systemctl --user start clipsync"
fi

if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    printf 'Add this directory to your PATH: %s\n' "$INSTALL_DIR"
fi

echo ""
echo "============================================"
echo "Installation completed!"
echo ""
echo "ClipSync has been installed to: ${BINARY}"
echo "Configuration file: ${CONFIG_DIR}/config.toml"
if [[ "$OS" == "linux" ]]; then
    echo "User systemd unit: $(clipsync_user_unit_path)"
    echo ""
    echo "Legacy system units are not removed automatically. Review before disabling:"
    echo "  sudo systemctl status clipsync"
else
    echo "Logs: ${CONFIG_DIR}/clipsync.{out,err}"
fi
echo ""
echo "To check status:"
echo "  clipsync status"
echo "  clipsync doctor"
echo ""
echo "To uninstall:"
echo "  ./scripts/uninstall_user.sh"
