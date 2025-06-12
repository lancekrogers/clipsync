# 📖 ClipSync User Guide

Complete guide to using ClipSync for seamless clipboard synchronization across your devices.

## 🎯 Table of Contents

1. [Getting Started](#-getting-started)
2. [First-Time Setup](#-first-time-setup)
3. [Daily Usage](#-daily-usage)
4. [Managing Devices](#-managing-devices)
5. [Clipboard History](#-clipboard-history)
6. [Global Hotkeys](#-global-hotkeys)
7. [Advanced Features](#-advanced-features)
8. [Common Workflows](#-common-workflows)
9. [Tips & Best Practices](#-tips--best-practices)

## 🚀 Getting Started

### What is ClipSync?

ClipSync automatically synchronizes your clipboard content across multiple devices on your local network. When you copy text, images, or other content on one device, it instantly becomes available on all your connected devices.

### Key Benefits

- **Instant Sync**: Copy on your laptop, paste on your desktop immediately
- **Secure**: Uses SSH key authentication and end-to-end encryption
- **Private**: No cloud services - everything stays on your local network
- **Smart**: Remembers your clipboard history with search capabilities
- **Fast**: Sub-500ms sync latency for most content

## 🔧 First-Time Setup

### Step 1: Install ClipSync

Follow the [Installation Guide](INSTALL.md) for your platform.

### Step 2: Generate SSH Keys

ClipSync uses SSH keys for secure authentication between devices:

```bash
# Generate a new key pair specifically for ClipSync
ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_clipsync -C "clipsync-$(hostname)"

# Or use your existing SSH key (ClipSync will find ~/.ssh/id_ed25519 automatically)
```

### Step 3: Initial Configuration

```bash
# Create default configuration
clipsync config init

# The config file will be created at:
# macOS: ~/Library/Application Support/clipsync/config.toml
# Linux: ~/.config/clipsync/config.toml
```

### Step 4: Start the Service

```bash
# Start ClipSync on your first device
clipsync start

# Check that it's running
clipsync status
```

### Step 5: Connect Your Second Device

On your second device, repeat steps 1-4, then connect the devices:

**Device 1:**
```bash
# Copy your public key
cat ~/.ssh/id_ed25519_clipsync.pub

# Add Device 2's public key (paste Device 2's public key)
clipsync auth add --name "laptop" --key "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI..."
```

**Device 2:**
```bash
# Copy your public key  
cat ~/.ssh/id_ed25519_clipsync.pub

# Add Device 1's public key (paste Device 1's public key)
clipsync auth add --name "desktop" --key "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI..."
```

### Step 6: Test the Connection

```bash
# Check connected peers
clipsync peers

# Test clipboard sync
echo "Hello from Device 1!" | clipsync copy
# On Device 2:
clipsync paste  # Should show "Hello from Device 1!"
```

## 📱 Daily Usage

### Basic Clipboard Operations

```bash
# Copy text to clipboard
clipsync copy "Your text here"
echo "Text from stdin" | clipsync copy

# Get current clipboard content
clipsync paste

# Clear clipboard
clipsync clear

# Force immediate sync across devices
clipsync sync
```

### Checking Status

```bash
# Show service status
clipsync status

# List connected devices
clipsync peers

# Show recent activity
clipsync logs --tail 10
```

### Service Management

```bash
# Start ClipSync
clipsync start

# Start in foreground (for debugging)
clipsync start --foreground

# Stop ClipSync
clipsync stop

# Restart service
clipsync restart
```

## 🔗 Managing Devices

### Adding New Devices

1. **Install ClipSync** on the new device
2. **Generate SSH keys** or use existing ones
3. **Exchange public keys** between devices:

```bash
# On new device, get public key
cat ~/.ssh/id_ed25519_clipsync.pub

# On existing devices, add the new device
clipsync auth add --name "new-device" --key "ssh-ed25519 AAAAC3..."

# On new device, add each existing device
clipsync auth add --name "existing-device" --key "ssh-ed25519 AAAAC3..."
```

### Managing Authorized Devices

```bash
# List all authorized devices
clipsync auth list

# Remove a device
clipsync auth remove --name "old-laptop"

# Show device details
clipsync auth show --name "desktop"
```

### Device Discovery

ClipSync can automatically discover other ClipSync devices on your network:

```bash
# Scan for ClipSync devices
clipsync peers --discover

# Show discovered but not connected devices
clipsync peers --available
```

## 📚 Clipboard History

### Viewing History

```bash
# Show recent clipboard history
clipsync history

# Show last 10 items
clipsync history --limit 10

# Interactive history browser
clipsync history --interactive
```

### Searching History

```bash
# Search for specific text
clipsync history --search "important note"

# Case-insensitive search
clipsync history --search "Password" --ignore-case

# Search by content type
clipsync history --type text
clipsync history --type image
```

### Managing History

```bash
# Clear specific history item
clipsync history --delete 3

# Clear all history
clipsync history --clear

# Export history to file
clipsync history --export ~/clipboard-backup.json

# Import history from file
clipsync history --import ~/clipboard-backup.json
```

## ⌨️ Global Hotkeys

ClipSync provides system-wide hotkeys for quick access:

### Default Hotkeys

| Hotkey | Action | Platform |
|--------|--------|----------|
| `Ctrl+Shift+V` | Show clipboard history | Linux |
| `Cmd+Shift+V` | Show clipboard history | macOS |
| `Ctrl+Shift+C` | Copy to secondary clipboard | Linux |
| `Cmd+Shift+C` | Copy to secondary clipboard | macOS |
| `Ctrl+Shift+S` | Force sync now | Linux |
| `Cmd+Shift+S` | Force sync now | macOS |
| `Ctrl+Shift+[` | Previous history item | Linux |
| `Cmd+Shift+[` | Previous history item | macOS |
| `Ctrl+Shift+]` | Next history item | Linux |
| `Cmd+Shift+]` | Next history item | macOS |

### Customizing Hotkeys

Edit your configuration file to customize hotkeys:

```toml
[hotkeys]
show_history = "Ctrl+Alt+V"        # Custom history hotkey
toggle_sync = "Ctrl+Alt+T"         # Toggle sync on/off
force_sync = "Ctrl+Alt+S"          # Force immediate sync
copy_secondary = "Ctrl+Alt+C"      # Copy to secondary clipboard
```

### Disabling Hotkeys

```toml
[hotkeys]
enabled = false  # Disable all hotkeys

# Or disable specific hotkeys
show_history = ""  # Empty string disables this hotkey
```

## 🔬 Advanced Features

### Clipboard Content Types

ClipSync supports various content types:

**Text Content:**
- Plain text (UTF-8)
- Rich text (RTF)
- HTML content
- Code snippets with syntax highlighting

**Binary Content:**
- Images (PNG, JPEG, TIFF)
- Files (small files up to 5MB)
- Custom binary data

**Large Content:**
- Automatic chunking for content >64KB
- Progress indicators for large transfers
- Compression for efficient transfer

### Configuration Profiles

Create different configuration profiles for different scenarios:

```bash
# Use custom config file
clipsync --config ~/work-config.toml start

# Create profile-specific configs
cp ~/.config/clipsync/config.toml ~/.config/clipsync/work.toml
cp ~/.config/clipsync/config.toml ~/.config/clipsync/home.toml

# Switch between profiles
clipsync --config ~/.config/clipsync/work.toml start
```

### Network Configuration

#### Custom Network Settings

```toml
# Custom listen address
listen_addr = "192.168.1.100:8484"

# Custom mDNS service name
advertise_name = "my-laptop-clipsync"

# Manual peer addresses (bypass discovery)
[[peers]]
name = "desktop"
address = "192.168.1.50:8484"
public_key = "ssh-ed25519 AAAAC3..."
```

#### Firewall Configuration

Ensure ClipSync can communicate through your firewall:

**Linux (ufw):**
```bash
sudo ufw allow 8484/tcp
```

**macOS:**
```bash
# Allow ClipSync through macOS firewall in System Preferences
```

### Advanced Security

#### Key Rotation

Regularly rotate your SSH keys for enhanced security:

```bash
# Generate new keys
ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_clipsync_new

# Update configuration to use new key
clipsync config edit

# Update authorized keys on all devices
clipsync auth add --name "this-device" --key "new-public-key"
clipsync auth remove --name "this-device-old"
```

#### Audit Logging

Enable detailed audit logging:

```toml
[logging]
level = "debug"
audit = true
audit_file = "~/.config/clipsync/audit.log"
```

## 🔄 Common Workflows

### Workflow 1: Developer Setup

Perfect for developers working across multiple machines:

```bash
# Terminal commands that sync across devices
git status | clipsync copy       # Copy git status to other machines
clipsync paste | grep "modified"  # Process synced content

# Sync code snippets
clipsync copy "kubectl get pods --all-namespaces"
# Switch to other machine and paste the command
```

### Workflow 2: Content Creation

For writers, designers, and content creators:

```bash
# Collect research across devices
clipsync history --search "research"  # Find all research clips
clipsync history --export research.json  # Export for backup

# Sync design assets
clipsync copy < image.png  # Copy image to other devices
```

### Workflow 3: System Administration

For managing multiple servers and workstations:

```bash
# Sync configuration snippets
cat nginx.conf | clipsync copy
# Apply on other servers

# Share command output
ps aux | grep nginx | clipsync copy
# Analyze on other machines
```

### Workflow 4: Presentations and Meetings

For presenters who switch between devices:

```bash
# Prepare presentation snippets
clipsync copy "https://important-meeting-link.com"
clipsync copy "Key presentation points: 1. Performance 2. Security 3. Scalability"

# Quick access during meetings
clipsync history --interactive  # Browse prepared content
```

## 💡 Tips & Best Practices

### Performance Tips

1. **Optimize for your network:**
   ```toml
   [clipboard]
   max_size = 1048576  # 1MB for slower networks
   ```

2. **Use compression for large content:**
   ```toml
   [security]
   compression = "zstd"  # Enable compression
   ```

3. **Limit history size for better performance:**
   ```toml
   [clipboard]
   history_size = 10  # Reduce from default 20
   ```

### Security Best Practices

1. **Use unique SSH keys for ClipSync:**
   ```bash
   ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_clipsync
   ```

2. **Regularly audit authorized devices:**
   ```bash
   clipsync auth list  # Review regularly
   ```

3. **Enable audit logging for sensitive environments:**
   ```toml
   [logging]
   audit = true
   ```

4. **Use firewall rules to restrict access:**
   ```bash
   # Only allow ClipSync from specific subnet
   sudo ufw allow from 192.168.1.0/24 to any port 8484
   ```

### Troubleshooting Tips

1. **Use foreground mode for debugging:**
   ```bash
   clipsync start --foreground
   ```

2. **Check logs regularly:**
   ```bash
   clipsync logs --tail 20
   ```

3. **Validate configuration:**
   ```bash
   clipsync config validate
   ```

4. **Run diagnostics:**
   ```bash
   clipsync doctor
   ```

### Productivity Tips

1. **Use descriptive device names:**
   ```bash
   clipsync auth add --name "work-laptop-2023" --key "..."
   ```

2. **Create command aliases:**
   ```bash
   alias ch='clipsync history --interactive'
   alias cs='clipsync sync'
   alias cp='clipsync paste'
   ```

3. **Integrate with your shell:**
   ```bash
   # Add to .bashrc/.zshrc
   function clip() {
       if [ $# -eq 0 ]; then
           clipsync paste
       else
           echo "$*" | clipsync copy
       fi
   }
   ```

4. **Use history search effectively:**
   ```bash
   # Create shortcuts for common searches
   alias clipcode='clipsync history --search "def\\|function\\|class"'
   alias clipurls='clipsync history --search "http"'
   ```

### Integration with Other Tools

**Git Integration:**
```bash
# Copy git commit messages
git log --oneline -5 | clipsync copy

# Share branch names
git branch --show-current | clipsync copy
```

**tmux Integration:**
```bash
# Copy tmux buffer to ClipSync
tmux show-buffer | clipsync copy

# Paste from ClipSync to tmux
clipsync paste | tmux load-buffer -
```

**vim/nvim Integration:**
Add to your `.vimrc`:
```vim
" Copy to ClipSync
vnoremap <leader>y :w !clipsync copy<CR>

" Paste from ClipSync
nnoremap <leader>p :r !clipsync paste<CR>
```

---

**Need more help?** 
- Check the [Troubleshooting Guide](TROUBLESHOOTING.md)
- See [Configuration Reference](CONFIG.md) for detailed settings
- Visit [GitHub Issues](https://github.com/yourusername/clipsync/issues) for community support