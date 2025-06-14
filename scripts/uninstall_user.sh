#!/bin/bash
# User-specific uninstallation script

set -e

echo "ClipSync User Uninstallation"
echo "============================"

# Stop and unload LaunchAgent
if [ -f ~/Library/LaunchAgents/com.clipsync.plist ]; then
    echo "Unloading LaunchAgent..."
    launchctl unload ~/Library/LaunchAgents/com.clipsync.plist 2>/dev/null || true
    rm -f ~/Library/LaunchAgents/com.clipsync.plist
    echo "✓ LaunchAgent removed"
fi

# Remove binary
if [ -f ~/.local/bin/clipsync ]; then
    rm -f ~/.local/bin/clipsync
    echo "✓ Binary removed"
fi

# Ask about config and data
echo ""
read -p "Remove configuration and data? (y/N) " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    rm -rf ~/.config/clipsync
    echo "✓ Configuration and data removed"
else
    echo "Configuration preserved at: ~/.config/clipsync"
fi

echo ""
echo "============================"
echo "Uninstallation completed!"
echo ""
echo "Note: PATH entries in shell configs were not removed."
echo "You can manually remove them from:"
echo "  ~/.zshrc"
echo "  ~/.bashrc"
echo "  ~/.bash_profile"