# ClipSync User Installation Guide (No sudo required)

This guide explains how to install ClipSync without requiring administrator privileges.

## Quick Start

```bash
# Clone the repository
git clone https://github.com/yourusername/clipsync.git
cd clipsync

# Run the user installation script
./install_user.sh
```

## What Gets Installed

The user installation places files in your home directory:

- **Binary**: `~/.local/bin/clipsync`
- **Config**: `~/.config/clipsync/config.toml`
- **Logs**: `~/.config/clipsync/clipsync.{out,err}`
- **LaunchAgent**: `~/Library/LaunchAgents/com.clipsync.plist` (macOS)

## Manual Installation Steps

If you prefer to install manually:

1. **Build the project**:
   ```bash
   cargo build --release
   ```

2. **Create directories**:
   ```bash
   mkdir -p ~/.local/bin
   mkdir -p ~/.config/clipsync
   ```

3. **Copy the binary**:
   ```bash
   cp target/release/clipsync ~/.local/bin/
   ```

4. **Generate config**:
   ```bash
   ~/.local/bin/clipsync config init > ~/.config/clipsync/config.toml
   ```

5. **Add to PATH**:
   ```bash
   echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
   source ~/.zshrc
   ```

## Auto-start on Login (macOS)

The installer creates a LaunchAgent that starts ClipSync when you log in. To manage it:

```bash
# Check status
launchctl list | grep clipsync

# Stop
launchctl unload ~/Library/LaunchAgents/com.clipsync.plist

# Start
launchctl load ~/Library/LaunchAgents/com.clipsync.plist
```

## Uninstallation

Run the uninstall script:
```bash
./uninstall_user.sh
```

Or manually remove:
```bash
rm -f ~/.local/bin/clipsync
rm -f ~/Library/LaunchAgents/com.clipsync.plist
rm -rf ~/.config/clipsync  # This removes config and data
```

## Advantages of User Installation

- ✅ No sudo/admin privileges required
- ✅ Easy to install and uninstall
- ✅ Config and data stay in your home directory
- ✅ Works with corporate/managed Macs
- ✅ Can be installed on shared systems

## Limitations

- Binary is only available to your user account
- Must ensure `~/.local/bin` is in your PATH
- On Linux, systemd user services require additional setup

## Troubleshooting

1. **Command not found**: Make sure `~/.local/bin` is in your PATH:
   ```bash
   echo $PATH | grep -q ".local/bin" || echo "PATH not configured"
   ```

2. **LaunchAgent not starting**: Check logs:
   ```bash
   tail -f ~/.config/clipsync/clipsync.err
   ```

3. **Permission denied**: Ensure you own all directories:
   ```bash
   ls -la ~/.local/bin/clipsync
   ls -la ~/.config/clipsync/
   ```