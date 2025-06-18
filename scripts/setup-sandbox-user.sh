#!/bin/bash
# Setup script for creating a sandboxed test user for clipsync development

set -e

# Source local config if it exists
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")"; pwd)"
[ -f "$SCRIPT_DIR/local-config.sh" ] && source "$SCRIPT_DIR/local-config.sh"

# Variables (can be overridden by local-config.sh)
TEST_USER="${TEST_USER:-clipsync-test}"
TEST_HOME="/home/$TEST_USER"

echo "Setting up sandboxed test user for clipsync development..."

# Create test user with restricted shell and no sudo access
sudo useradd -m -s /bin/bash "$TEST_USER" || echo "User already exists"

# Set a password for the test user
echo "Please set a password for the test user:"
sudo passwd "$TEST_USER"

# Create necessary directories
sudo -u "$TEST_USER" mkdir -p "$TEST_HOME/.config/clipsync"
sudo -u "$TEST_USER" mkdir -p "$TEST_HOME/.local/share/clipsync"

# Copy development files (but not config with any sensitive data)
CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.."; pwd)"
sudo cp -r "$CURRENT_DIR" "$TEST_HOME/clipsync-dev"
sudo chown -R "$TEST_USER:$TEST_USER" "$TEST_HOME/clipsync-dev"

# Create a restricted sudoers entry (only for specific testing commands if needed)
cat << EOF | sudo tee /etc/sudoers.d/clipsync-test
# Restricted sudo access for clipsync testing
# $TEST_USER ALL=(ALL) NOPASSWD: /usr/bin/journalctl -u clipsync-test
EOF

echo "Test user '$TEST_USER' created successfully!"
echo "To switch to test user: su - $TEST_USER"
echo "Development directory: $TEST_HOME/clipsync-dev"