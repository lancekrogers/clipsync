#!/usr/bin/env bash
# User-specific uninstallation script

set -euo pipefail

echo "ClipSync User Uninstallation"
echo "============================"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ "$OSTYPE" == "darwin"* ]]; then
    OS="macos"
elif [[ "$OSTYPE" == "linux-gnu"* ]] || [[ "$(uname -s)" == "Linux" ]]; then
    OS="linux"
else
    OS="unknown"
fi

if [[ "$OS" == "macos" ]]; then
    if [ -f "${HOME}/Library/LaunchAgents/com.clipsync.plist" ]; then
        echo "Unloading LaunchAgent..."
        launchctl bootout "gui/$(id -u)" "${HOME}/Library/LaunchAgents/com.clipsync.plist" 2>/dev/null \
            || launchctl unload "${HOME}/Library/LaunchAgents/com.clipsync.plist" 2>/dev/null || true
        rm -f "${HOME}/Library/LaunchAgents/com.clipsync.plist"
        echo "LaunchAgent removed"
    fi
elif [[ "$OS" == "linux" ]]; then
    # shellcheck source=lib/linux_user_service.sh
    source "$SCRIPT_DIR/lib/linux_user_service.sh"
    UNIT_PATH="$(clipsync_user_unit_path)"
    if [[ -f "$UNIT_PATH" ]]; then
        echo "Disabling user systemd service (system units are untouched)..."
        clipsync_systemctl disable clipsync.service
        clipsync_systemctl stop clipsync.service
        rm -f "$UNIT_PATH"
        clipsync_reload_user_systemd
        echo "User systemd unit removed"
    fi
fi

INSTALL_DIR="${CLIPSYNC_INSTALL_DIR:-${HOME}/.local/bin}"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/clipsync"
if [ -f "$INSTALL_DIR/clipsync" ]; then
    rm -f "$INSTALL_DIR/clipsync"
    echo "Binary removed"
fi

echo ""
read -p "Remove configuration? (y/N) " -n 1 -r || REPLY=""
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    rm -rf "$CONFIG_DIR"
    echo "Configuration removed; clipboard history remains in its configured data directory"
else
    echo "Configuration preserved at: $CONFIG_DIR"
fi

echo ""
echo "============================"
echo "Uninstallation completed!"
echo ""
if [[ "$OS" == "linux" ]]; then
    echo "If a legacy system-wide clipsync.service remains, review it manually:"
    echo "  sudo systemctl status clipsync"
fi
echo ""
echo "Note: PATH entries in shell configs were not removed."
echo "You can manually remove them from:"
echo "  ~/.zshrc"
echo "  ~/.bashrc"
echo "  ~/.bash_profile"
