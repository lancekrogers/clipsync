#!/bin/bash
# Install and enable AppArmor profile for clipsync

set -e

echo "Installing AppArmor profile for clipsync..."

# Install AppArmor if not already installed
sudo pacman -S --needed apparmor

# Enable AppArmor service
sudo systemctl enable --now apparmor

# Copy profile to AppArmor directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")"; pwd)"
sudo cp "$SCRIPT_DIR/apparmor-profile" /etc/apparmor.d/usr.local.bin.clipsync

# Load the profile
sudo apparmor_parser -r /etc/apparmor.d/usr.local.bin.clipsync

# Set profile to enforce mode
sudo aa-enforce /usr/local/bin/clipsync

echo "AppArmor profile installed and enforced!"
echo "To check status: sudo aa-status | grep clipsync"
echo "To disable temporarily: sudo aa-disable /usr/local/bin/clipsync"
echo "To set to complain mode: sudo aa-complain /usr/local/bin/clipsync"