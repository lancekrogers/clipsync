#!/bin/bash
# User-specific installation script (no sudo required)

set -e

echo "ClipSync User Installation (No sudo required)"
echo "============================================"

# Detect OS
if [[ "$OSTYPE" == "darwin"* ]]; then
    OS="macos"
else
    echo "This user installation script is currently only for macOS"
    echo "Linux still requires sudo for systemd service installation"
    exit 1
fi

# Create user directories
echo "Creating user directories..."
mkdir -p ~/.local/bin
mkdir -p ~/.config/clipsync
mkdir -p ~/Library/LaunchAgents

# Build if not already built
if [ ! -f "target/release/clipsync" ]; then
    echo "Building ClipSync..."
    cargo build --release
fi

# Copy binary
echo "Installing binary to ~/.local/bin/..."
cp target/release/clipsync ~/.local/bin/

# Create user-specific LaunchAgent
echo "Creating LaunchAgent..."
cat > ~/Library/LaunchAgents/com.clipsync.plist << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.clipsync</string>
    <key>ProgramArguments</key>
    <array>
        <string>$HOME/.local/bin/clipsync</string>
        <string>start</string>
        <string>--daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$HOME/.config/clipsync/clipsync.out</string>
    <key>StandardErrorPath</key>
    <string>$HOME/.config/clipsync/clipsync.err</string>
    <key>WorkingDirectory</key>
    <string>$HOME</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin</string>
    </dict>
</dict>
</plist>
EOF

# Generate default config if it doesn't exist
if [ ! -f ~/.config/clipsync/config.toml ]; then
    echo "Generating default configuration..."
    ~/.local/bin/clipsync config init > ~/.config/clipsync/config.toml
fi

# Update PATH in shell config
echo ""
echo "Updating PATH in shell configuration..."

# Function to add path to shell config
add_to_path() {
    local shell_config=$1
    local path_line='export PATH="$HOME/.local/bin:$PATH"'
    
    if [ -f "$shell_config" ] && [ -w "$shell_config" ]; then
        if ! grep -q ".local/bin" "$shell_config"; then
            echo "" >> "$shell_config"
            echo "# Added by ClipSync installer" >> "$shell_config"
            echo "$path_line" >> "$shell_config"
            echo "✓ Updated $shell_config"
        else
            echo "✓ PATH already configured in $shell_config"
        fi
    elif [ -f "$shell_config" ]; then
        echo "⚠ Cannot write to $shell_config (permission denied)"
    fi
}

# Update various shell configs
add_to_path ~/.zshrc
add_to_path ~/.bashrc
add_to_path ~/.bash_profile

# Load LaunchAgent
echo ""
echo "Loading LaunchAgent..."
launchctl load ~/Library/LaunchAgents/com.clipsync.plist

echo ""
echo "============================================"
echo "Installation completed!"
echo ""
echo "ClipSync has been installed to: ~/.local/bin/clipsync"
echo "Configuration file: ~/.config/clipsync/config.toml"
echo "Logs: ~/.config/clipsync/clipsync.{out,err}"
echo ""
echo "To use clipsync command immediately, run:"
echo "  source ~/.zshrc"
echo "Or start a new terminal session"
echo ""
echo "To check status:"
echo "  clipsync status"
echo ""
echo "To uninstall:"
echo "  ./uninstall_user.sh"