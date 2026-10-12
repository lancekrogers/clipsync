# ClipSync User Installation Guide (No sudo required)

This guide explains how to install ClipSync without requiring administrator privileges.

## Quick Start

```bash
# Clone the repository
git clone https://github.com/lancekrogers/clipsync.git
cd clipsync

# Run the user installation script (macOS and Linux)
make install-user
# or: ./scripts/install_user.sh
```

## What Gets Installed

The user installation places files in your home directory:

- **Binary**: `~/.local/bin/clipsync`
- **Config**: `${XDG_CONFIG_HOME:-~/.config}/clipsync/config.toml` for the source installer
- **Logs**: `~/.config/clipsync/clipsync.{out,err}`
- **LaunchAgent**: `~/Library/LaunchAgents/com.clipsync.plist` (macOS)
- **User systemd unit**: `~/.config/systemd/user/clipsync.service` (Linux, graphical session)

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
   ~/.local/bin/clipsync config init
   ```

5. **Add to PATH**:
   ```bash
   echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
   source ~/.zshrc
   ```

## Auto-start on Login

### macOS

The installer creates a LaunchAgent that starts ClipSync when you log in. To manage it:

```bash
# Check status
launchctl list | grep clipsync

# Stop
launchctl unload ~/Library/LaunchAgents/com.clipsync.plist

# Start
launchctl load ~/Library/LaunchAgents/com.clipsync.plist
```

### Linux (systemd user service)

Supported installs use a **per-user** unit tied to `graphical-session.target` and run `clipsync start --foreground`. No root clipboard daemon is created.

```bash
systemctl --user daemon-reload
systemctl --user enable --now clipsync
systemctl --user status clipsync
clipsync doctor
```

If you previously installed a **system-wide** `clipsync.service`, ClipSync does not remove or stop it automatically. Review and migrate manually:

```bash
sudo systemctl status clipsync   # legacy system unit, if present
systemctl --user status clipsync # supported user unit
```

## Uninstallation

Run the uninstall script:
```bash
./scripts/uninstall_user.sh
```

Or manually remove:
```bash
rm -f ~/.local/bin/clipsync
rm -f ~/Library/LaunchAgents/com.clipsync.plist
rm -rf ~/.config/clipsync  # Optional: removes configuration, not the history store
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
- On Linux, the user service starts with your graphical session; headless SSH sessions do not provide a desktop clipboard. Start the service from the graphical session so its user manager has the correct display environment

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
The source installer accepts `CLIPSYNC_INSTALL_DIR` and the standard XDG config/data directories. The uninstaller uses the same install/config overrides and preserves configuration by default. Installation does not create or authorize a device identity; complete the README pairing steps before starting sync. Existing root services require an explicit migration decision.

`print-user-unit --binary /absolute/path/to/clipsync` generates a unit without loading configuration. Downloaded installation requires a release containing this command; older release binaries cannot provide the new unit generator. GitHub release workflows are currently disabled, so this PR does not publish a new binary.

## Installer verification

Run `just test-install` with Docker to exercise source install/reinstall/uninstall,
the downloaded installer with a local archive fixture, generated tarball installs,
unit parsing by systemd, and failure propagation. This uses temporary directories
and a mock user service manager. It does not test graphical login/logout or migrate
an existing installation.

The Linux unit generator accepts spaces, Unicode, dollar signs and percent signs
in absolute executable paths. It rejects quotes, backslashes and control characters
because supported systemd versions reject these in executable names.
