# ClipSync Installation Guide

## Prerequisites

### macOS
- macOS 11.0 or later
- Xcode Command Line Tools: `xcode-select --install`
- (Optional) Homebrew for easier installation

### Arch Linux
- X11 or Wayland display server
- Required packages:
  ```bash
  sudo pacman -S libx11 libxcb openssl
  ```

## Building from Source

### 1. Clone the repository
```bash
git clone https://github.com/yourusername/clipsync.git
cd clipsync
```

### 2. Build the project
```bash
# For current platform
make build

# For release build
make release
```

## Installation

### macOS

#### Option 1: User Installation (No sudo required) - Recommended
```bash
# Run the user installation script
./install_user.sh

# Or manually:
mkdir -p ~/.local/bin
cp target/release/clipsync ~/.local/bin/
~/.local/bin/clipsync config init > ~/.config/clipsync/config.toml

# Add to PATH in your shell config
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

#### Option 2: System-wide Installation (Requires sudo)
```bash
# Build the release binary
make release

# Copy binary to system path
sudo cp target/release/clipsync /usr/local/bin/

# Install LaunchAgent (for auto-start)
cp scripts/com.clipsync.plist ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/com.clipsync.plist
```

#### Option 3: Using Make (User installation)
```bash
make install
```

### Arch Linux

#### Option 1: Manual Installation
```bash
# Build the release binary
make release

# Copy binary to system path
sudo cp target/release/clipsync /usr/local/bin/

# Install systemd service
sudo cp scripts/clipsync.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable clipsync
sudo systemctl start clipsync
```

#### Option 2: Using Make
```bash
sudo make install
```

## Configuration

1. Generate a default configuration file:
   ```bash
   clipsync config init > ~/.config/clipsync/config.toml
   ```

2. Edit the configuration as needed:
   ```bash
   $EDITOR ~/.config/clipsync/config.toml
   ```

3. Key configuration options:
   - `port`: The port to listen on (default: 9090)
   - `max_history_size`: Number of clipboard items to keep
   - `enable_encryption`: Enable/disable encryption
   - `auto_start`: Start syncing on launch

## Testing the Installation

1. Check the version:
   ```bash
   clipsync --version
   ```

2. Test clipboard copy:
   ```bash
   echo "Hello, ClipSync!" | clipsync copy
   ```

3. Test clipboard paste:
   ```bash
   clipsync paste
   ```

4. Check service status:
   ```bash
   clipsync status
   ```

5. View connected peers:
   ```bash
   clipsync peers
   ```

## Setting Up Sync Between Devices

1. Install ClipSync on both devices following the above instructions

2. Ensure both devices are on the same network

3. Start the service on both devices:
   ```bash
   clipsync start
   ```

4. The devices should automatically discover each other via mDNS

5. For manual peer configuration, edit the config file:
   ```toml
   [[manual_peers]]
   id = "device-name"
   address = "192.168.1.100:9090"
   ```

## Troubleshooting

### macOS Issues

1. **Permission errors**: Grant Terminal full disk access in System Preferences > Security & Privacy

2. **LaunchAgent not starting**: Check logs with:
   ```bash
   log show --predicate 'process == "clipsync"' --last 1h
   ```

3. **Clipboard access denied**: Ensure the app has accessibility permissions

### Arch Linux Issues

1. **X11 clipboard errors**: Install `xclip` or `xsel`:
   ```bash
   sudo pacman -S xclip
   ```

2. **Systemd service fails**: Check logs with:
   ```bash
   journalctl -u clipsync -f
   ```

3. **Permission denied**: Ensure your user is in the correct groups:
   ```bash
   sudo usermod -a -G input $USER
   ```

## Uninstallation

### macOS
```bash
make uninstall
# or manually:
launchctl unload ~/Library/LaunchAgents/com.clipsync.plist
rm ~/Library/LaunchAgents/com.clipsync.plist
sudo rm /usr/local/bin/clipsync
rm -rf ~/.config/clipsync
```

### Arch Linux
```bash
sudo make uninstall
# or manually:
sudo systemctl stop clipsync
sudo systemctl disable clipsync
sudo rm /etc/systemd/system/clipsync.service
sudo rm /usr/local/bin/clipsync
rm -rf ~/.config/clipsync
```

## Security Considerations

1. ClipSync uses SSH keys for authentication between peers
2. All clipboard data is encrypted in transit using AES-256-GCM
3. History is stored encrypted on disk
4. First-time setup will generate SSH keys automatically

## Next Steps

- Run `clipsync --help` to see all available commands
- Check the [README](README.md) for more detailed usage information
- Report issues at: https://github.com/yourusername/clipsync/issues