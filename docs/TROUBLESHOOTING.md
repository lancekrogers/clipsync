# 🩺 ClipSync Troubleshooting Guide

Comprehensive guide to diagnosing and fixing common ClipSync issues.

## 🚀 Quick Diagnostics

### First Steps

When ClipSync isn't working as expected, start with these quick checks:

```bash
# 1. Check if ClipSync is running
clipsync status

# 2. Validate your configuration
clipsync config validate

# 3. Run built-in diagnostics
clipsync doctor

# 4. Check recent logs
clipsync logs --tail 20

# 5. Test basic functionality
echo "test" | clipsync copy
clipsync paste
```

### Emergency Reset

If ClipSync is completely broken, try this reset sequence:

```bash
# Stop ClipSync
clipsync stop

# Reset configuration to defaults
clipsync config init --force

# Clear history database
rm ~/.local/share/clipsync/history.db      # Linux
rm ~/Library/Application\ Support/clipsync/history.db  # macOS

# Restart ClipSync
clipsync start --foreground
```

## 🔧 Installation Issues

### ClipSync Command Not Found

**Problem:** `clipsync: command not found`

**Solutions:**

1. **Check if installed:**
   ```bash
   which clipsync
   ls -la /usr/local/bin/clipsync
   ```

2. **Update PATH (if installed manually):**
   ```bash
   echo 'export PATH="/usr/local/bin:$PATH"' >> ~/.bashrc
   source ~/.bashrc
   ```

3. **Reinstall ClipSync:**
   ```bash
   # macOS
   brew reinstall clipsync
   
   # Linux (manual)
   sudo rm /usr/local/bin/clipsync
   # Re-download and install
   ```

### Permission Denied

**Problem:** `Permission denied` when running clipsync

**Solutions:**

1. **Fix binary permissions:**
   ```bash
   sudo chmod +x /usr/local/bin/clipsync
   ```

2. **macOS Gatekeeper issues:**
   ```bash
   # Remove quarantine attribute
   sudo xattr -d com.apple.quarantine /usr/local/bin/clipsync
   
   # Or allow in System Preferences > Security & Privacy
   ```

3. **Linux AppArmor/SELinux:**
   ```bash
   # Check for security restrictions
   dmesg | grep -i denied
   journalctl | grep -i apparmor
   ```

### Missing Dependencies

**Problem:** ClipSync crashes with library errors

**Linux Solutions:**
```bash
# Check missing libraries
ldd $(which clipsync)

# Install missing dependencies
# Ubuntu/Debian
sudo apt install libx11-6 libxcb1 libssl3

# Fedora/RHEL
sudo dnf install libX11 libxcb openssl

# Arch Linux
sudo pacman -S libx11 libxcb openssl
```

**macOS Solutions:**
```bash
# Update system
sudo softwareupdate -i -a

# Reinstall Xcode Command Line Tools
sudo xcode-select --install
```

## 🌐 Connection Problems

### Devices Can't Find Each Other

**Problem:** Devices don't appear in discovery or can't connect

**Diagnosis:**
```bash
# Check network discovery
clipsync peers --discover

# Test network connectivity
ping other-device-ip
telnet other-device-ip 8484

# Check firewall
sudo ufw status              # Linux
sudo iptables -L            # Linux (detailed)
```

**Solutions:**

1. **Check firewall settings:**
   ```bash
   # Linux (ufw)
   sudo ufw allow 8484/tcp
   
   # Linux (iptables)
   sudo iptables -A INPUT -p tcp --dport 8484 -j ACCEPT
   
   # macOS - Allow ClipSync in System Preferences > Security > Firewall
   ```

2. **Verify both devices are on same network:**
   ```bash
   # Check IP addresses
   ip addr show                # Linux
   ifconfig                    # macOS
   
   # Ensure both devices are in same subnet (e.g., 192.168.1.x)
   ```

3. **Check mDNS/Bonjour service:**
   ```bash
   # Linux - install Avahi
   sudo apt install avahi-daemon    # Ubuntu/Debian
   sudo dnf install avahi           # Fedora
   sudo systemctl start avahi-daemon
   
   # macOS - Bonjour should be built-in
   dns-sd -B _clipsync._tcp local.
   ```

4. **Manual peer configuration:**
   ```toml
   # Add to config.toml
   [[peers]]
   name = "other-device"
   address = "192.168.1.50:8484"  # Replace with actual IP
   public_key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI..."
   ```

### Connection Timeouts

**Problem:** Devices connect but then timeout

**Diagnosis:**
```bash
# Check connection status
clipsync status

# Monitor connection in real-time
clipsync start --foreground

# Check network latency
ping other-device-ip
```

**Solutions:**

1. **Adjust timeout settings:**
   ```toml
   # In config.toml
   timeout_connect = "60s"      # Increase connection timeout
   timeout_handshake = "30s"    # Increase handshake timeout
   keepalive_interval = "15s"   # More frequent keepalives
   ```

2. **Check network stability:**
   ```bash
   # Test sustained connectivity
   ping -c 100 other-device-ip
   
   # Check for packet loss
   mtr other-device-ip
   ```

3. **Reduce payload size for slow networks:**
   ```toml
   [clipboard]
   max_size = 1_048_576  # 1MB instead of 5MB
   ```

### SSL/TLS Errors

**Problem:** Certificate or encryption errors

**Solutions:**

1. **Regenerate SSH keys:**
   ```bash
   # Create new key pair
   ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_clipsync_new
   
   # Update configuration
   clipsync config edit
   # Change ssh_key path to new key
   
   # Re-exchange public keys between devices
   ```

2. **Check key permissions:**
   ```bash
   chmod 600 ~/.ssh/id_ed25519*
   chmod 644 ~/.ssh/id_ed25519*.pub
   ```

3. **Verify key format:**
   ```bash
   # Check key is valid
   ssh-keygen -l -f ~/.ssh/id_ed25519.pub
   
   # Check key type
   file ~/.ssh/id_ed25519
   ```

## 🔐 Authentication Issues

### Authentication Failed

**Problem:** Devices connect but authentication fails

**Diagnosis:**
```bash
# Check authorized keys
clipsync auth list

# Verify public key format
cat ~/.ssh/id_ed25519.pub

# Check logs for auth errors
clipsync logs | grep -i auth
```

**Solutions:**

1. **Re-exchange public keys:**
   ```bash
   # On Device A
   cat ~/.ssh/id_ed25519.pub
   # Copy this output
   
   # On Device B
   clipsync auth add --name "device-a" --key "ssh-ed25519 AAAAC3..."
   
   # Repeat in reverse direction
   ```

2. **Check authorized_keys file:**
   ```bash
   # View authorized keys
   cat ~/.config/clipsync/authorized_keys
   
   # Manually add key if needed
   echo "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI... device-name" >> ~/.config/clipsync/authorized_keys
   ```

3. **Generate new SSH keys:**
   ```bash
   # Create ClipSync-specific keys
   ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_clipsync -C "clipsync-$(hostname)"
   
   # Update config to use new key
   clipsync config edit
   ```

### Key Format Errors

**Problem:** "Invalid key format" errors

**Solutions:**

1. **Check key file exists:**
   ```bash
   ls -la ~/.ssh/id_ed25519*
   ```

2. **Verify key format:**
   ```bash
   # Should start with "ssh-ed25519"
   head -1 ~/.ssh/id_ed25519.pub
   
   # Should be single line
   wc -l ~/.ssh/id_ed25519.pub
   ```

3. **Regenerate corrupted keys:**
   ```bash
   # Backup old keys
   mv ~/.ssh/id_ed25519 ~/.ssh/id_ed25519.backup
   mv ~/.ssh/id_ed25519.pub ~/.ssh/id_ed25519.pub.backup
   
   # Generate new keys
   ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519
   ```

## 📋 Clipboard Issues

### Clipboard Not Syncing

**Problem:** Changes don't sync between devices

**Diagnosis:**
```bash
# Test basic clipboard operations
echo "test sync" | clipsync copy
clipsync paste

# Check clipboard monitoring
clipsync start --foreground
# Copy something and watch logs

# Force manual sync
clipsync sync
```

**Solutions:**

1. **Check clipboard permissions:**
   ```bash
   # macOS - Grant accessibility permissions in System Preferences
   # System Preferences > Security & Privacy > Privacy > Accessibility
   # Add ClipSync to allowed applications
   
   # Linux - Check X11/Wayland access
   echo $DISPLAY
   echo $WAYLAND_DISPLAY
   ```

2. **Platform-specific fixes:**

   **macOS:**
   ```bash
   # Reset clipboard permissions
   tccutil reset Accessibility com.clipsync.app
   
   # Check for competing clipboard managers
   ps aux | grep -i clipboard
   ```

   **Linux X11:**
   ```bash
   # Install xclip/xsel if missing
   sudo apt install xclip xsel      # Ubuntu/Debian
   sudo dnf install xclip xsel      # Fedora
   
   # Test clipboard access
   echo "test" | xclip -selection clipboard
   xclip -selection clipboard -o
   ```

   **Linux Wayland:**
   ```bash
   # Install wl-clipboard
   sudo apt install wl-clipboard    # Ubuntu/Debian
   sudo dnf install wl-clipboard    # Fedora
   
   # Test clipboard access
   echo "test" | wl-copy
   wl-paste
   ```

3. **Adjust polling settings:**
   ```toml
   [clipboard]
   sync_interval = "500ms"  # Less frequent polling
   # or
   sync_interval = "50ms"   # More frequent polling
   ```

### Large Content Issues

**Problem:** Large images or files don't sync

**Solutions:**

1. **Increase size limits:**
   ```toml
   [clipboard]
   max_size = 20_971_520  # 20MB instead of 5MB
   ```

2. **Enable compression:**
   ```toml
   [security]
   compression = "zstd"
   compression_level = 5
   ```

3. **Check available memory:**
   ```bash
   free -h                 # Linux
   vm_stat                 # macOS
   ```

### Content Type Issues

**Problem:** Certain content types don't sync

**Solutions:**

1. **Check allowed MIME types:**
   ```toml
   [clipboard]
   allowed_mime_types = [
       "text/plain",
       "text/html",
       "text/rtf",
       "image/png",
       "image/jpeg",
       "image/tiff",
       "application/octet-stream"  # Add if needed
   ]
   ```

2. **Debug content detection:**
   ```bash
   # Start with verbose logging
   RUST_LOG=debug clipsync start --foreground
   
   # Copy content and check logs for MIME type detection
   ```

## ⌨️ Hotkey Issues

### Hotkeys Not Working

**Problem:** Global hotkeys don't respond

**Diagnosis:**
```bash
# Check hotkey configuration
clipsync config show | grep -A 10 "\[hotkeys\]"

# Test hotkey registration
clipsync start --foreground
# Watch for hotkey registration messages
```

**Solutions:**

1. **Check hotkey conflicts:**
   ```bash
   # macOS - Check System Preferences > Keyboard > Shortcuts
   # Look for conflicts with ClipSync hotkeys
   
   # Linux - Check other applications using same hotkeys
   ps aux | grep -i hotkey
   ```

2. **Fix hotkey permissions:**

   **macOS:**
   ```bash
   # Grant Accessibility permissions
   # System Preferences > Security & Privacy > Privacy > Accessibility
   # Enable ClipSync
   ```

   **Linux:**
   ```bash
   # Check if running in correct session
   echo $XDG_SESSION_TYPE
   echo $DISPLAY
   
   # Ensure ClipSync has access to input devices
   groups $USER | grep input
   ```

3. **Try alternative hotkeys:**
   ```toml
   [hotkeys]
   show_history = "Ctrl+Alt+V"     # Different combination
   toggle_sync = "Ctrl+Alt+T"
   ```

4. **Disable conflicting applications:**
   ```bash
   # Temporarily disable other clipboard managers
   killall clipmenu                # Linux
   killall ClipMenu               # macOS
   ```

### Hotkey Registration Fails

**Problem:** ClipSync can't register global hotkeys

**Solutions:**

1. **Run with elevated privileges (temporary test):**
   ```bash
   sudo clipsync start --foreground
   # If this works, it's a permissions issue
   ```

2. **Check system hotkey limits:**
   ```bash
   # Linux - Check systemd user services
   systemctl --user status
   
   # macOS - Check Console app for errors
   ```

3. **Disable hotkeys temporarily:**
   ```toml
   [hotkeys]
   enabled = false
   ```

## 📁 File and Service Issues

### Service Won't Start

**Problem:** ClipSync service fails to start

**Diagnosis:**
```bash
# Check what's preventing startup
clipsync start --foreground

# Check for conflicting processes
ps aux | grep clipsync
lsof -i :8484

# Check system logs
journalctl --user -u clipsync    # Linux
log show --predicate 'process == "clipsync"'  # macOS
```

**Solutions:**

1. **Kill conflicting processes:**
   ```bash
   # Find processes using ClipSync port
   lsof -i :8484
   
   # Kill specific process
   kill -9 <PID>
   
   # Or use different port
   # Edit config.toml: listen_addr = ":8485"
   ```

2. **Fix file permissions:**
   ```bash
   # Ensure config directory is accessible
   chmod 755 ~/.config/clipsync
   chmod 644 ~/.config/clipsync/config.toml
   
   # Check log file permissions
   touch ~/.config/clipsync/clipsync.log
   chmod 644 ~/.config/clipsync/clipsync.log
   ```

3. **Clear corrupted state:**
   ```bash
   # Remove PID files
   rm ~/.config/clipsync/clipsync.pid
   
   # Clear lock files
   rm ~/.config/clipsync/*.lock
   ```

### Database Errors

**Problem:** History database corruption or access errors

**Solutions:**

1. **Backup and reset database:**
   ```bash
   # Backup existing database
   cp ~/.local/share/clipsync/history.db ~/.local/share/clipsync/history.db.backup
   
   # Remove corrupted database
   rm ~/.local/share/clipsync/history.db
   
   # Restart ClipSync (will create new database)
   clipsync restart
   ```

2. **Check disk space:**
   ```bash
   df -h ~/.local/share/clipsync/    # Linux
   df -h ~/Library/Application\ Support/clipsync/  # macOS
   ```

3. **Fix database permissions:**
   ```bash
   chmod 600 ~/.local/share/clipsync/history.db
   chown $USER ~/.local/share/clipsync/history.db
   ```

### Configuration Errors

**Problem:** Invalid configuration prevents startup

**Diagnosis:**
```bash
# Validate configuration
clipsync config validate

# Check configuration syntax
cat ~/.config/clipsync/config.toml | grep -n "="
```

**Solutions:**

1. **Reset to default configuration:**
   ```bash
   # Backup current config
   cp ~/.config/clipsync/config.toml ~/.config/clipsync/config.toml.backup
   
   # Create default config
   clipsync config init --force
   
   # Gradually add back customizations
   ```

2. **Fix TOML syntax errors:**
   ```bash
   # Common issues:
   # - Missing quotes around strings
   # - Incorrect boolean values (true/false, not True/False)
   # - Invalid characters in section names
   
   # Use online TOML validator or:
   python3 -c "import toml; toml.load('~/.config/clipsync/config.toml')"
   ```

## 🔍 Performance Issues

### High CPU Usage

**Problem:** ClipSync using excessive CPU

**Diagnosis:**
```bash
# Monitor ClipSync CPU usage
top -p $(pgrep clipsync)        # Linux
top | grep clipsync             # macOS

# Check for polling issues
clipsync logs | grep -i poll
```

**Solutions:**

1. **Adjust polling frequency:**
   ```toml
   [clipboard]
   sync_interval = "1s"         # Less frequent polling
   
   [performance]
   adaptive_polling = true      # Enable adaptive polling
   min_poll_interval = "100ms"
   max_poll_interval = "5s"
   ```

2. **Reduce worker threads:**
   ```toml
   [performance]
   worker_threads = 2           # Reduce from default 4
   ```

3. **Disable unnecessary features:**
   ```toml
   [hotkeys]
   enabled = false              # Disable if not needed
   
   [clipboard]
   history_size = 5             # Reduce history size
   ```

### High Memory Usage

**Problem:** ClipSync consuming too much memory

**Solutions:**

1. **Limit memory usage:**
   ```toml
   [performance]
   max_memory_usage = "50MB"    # Set memory limit
   
   [clipboard]
   max_size = 1_048_576         # Reduce max clipboard size
   history_size = 10            # Reduce history
   ```

2. **Clear history database:**
   ```bash
   clipsync history --clear
   ```

3. **Check for memory leaks:**
   ```bash
   # Monitor memory usage over time
   while true; do
       ps -o pid,vsz,rss,comm -p $(pgrep clipsync)
       sleep 60
   done
   ```

### Slow Sync Performance

**Problem:** Clipboard sync is slow

**Solutions:**

1. **Optimize network settings:**
   ```toml
   [performance]
   tcp_nodelay = true
   socket_buffer_size = 262144  # 256KB buffer
   
   [security]
   compression = "zstd"
   compression_level = 1        # Fast compression
   ```

2. **Check network quality:**
   ```bash
   # Test bandwidth to other device
   iperf3 -s                   # On one device
   iperf3 -c other-device-ip   # On other device
   ```

3. **Reduce payload size:**
   ```toml
   [clipboard]
   max_size = 1_048_576        # 1MB limit for faster sync
   ```

## 📊 Logging and Debugging

### Enable Debug Logging

```bash
# Temporary debug logging
RUST_LOG=debug clipsync start --foreground

# Persistent debug logging
clipsync config edit
# Set: log_level = "debug"

# Module-specific logging
RUST_LOG=clipsync::transport=debug,clipsync::auth=info clipsync start --foreground
```

### Log Analysis

```bash
# Show recent errors
clipsync logs | grep -i error

# Show authentication events
clipsync logs | grep -i auth

# Show network events
clipsync logs | grep -i transport

# Follow logs in real-time
clipsync logs --follow

# Export logs for support
clipsync logs --export ~/clipsync-debug.log
```

### Common Log Messages

**Normal Operation:**
```
INFO ClipSync started successfully
INFO Connected to peer 'laptop' at 192.168.1.50
INFO Clipboard synced successfully (142 bytes)
```

**Warning Messages:**
```
WARN Failed to connect to peer 'desktop': connection timeout
WARN Large clipboard content detected (3.2MB), compression enabled
WARN Hotkey registration failed: Ctrl+Shift+V already in use
```

**Error Messages:**
```
ERROR Authentication failed: invalid public key
ERROR Clipboard access denied: missing permissions
ERROR Database error: disk full
```

## ❓ Frequently Asked Questions

### Q: ClipSync says "running" but clipboard doesn't sync

**A:** This usually indicates a connectivity or authentication issue:

1. Check if devices can see each other: `clipsync peers --discover`
2. Verify authentication: `clipsync auth list`
3. Test manual sync: `clipsync sync`
4. Check logs: `clipsync logs | grep -i error`

### Q: Hotkeys work sometimes but not always

**A:** This is often a focus or permission issue:

1. Ensure ClipSync has Accessibility permissions (macOS)
2. Check for conflicting applications using same hotkeys
3. Try alternative hotkey combinations
4. Restart ClipSync: `clipsync restart`

### Q: Large images don't sync

**A:** Check size limits and compression:

1. Increase max size: `max_size = 20_971_520` (20MB)
2. Enable compression: `compression = "zstd"`
3. Check available memory and disk space
4. Monitor logs during large transfers

### Q: ClipSync stops working after sleep/hibernate

**A:** This is a network connectivity issue:

1. ClipSync should auto-reconnect, wait 30-60 seconds
2. Force reconnection: `clipsync sync`
3. Restart if needed: `clipsync restart`
4. Check network connectivity: `ping other-device`

### Q: Getting "port already in use" error

**A:** Another service is using ClipSync's port:

1. Find conflicting process: `lsof -i :8484`
2. Kill process or use different port
3. Change port in config: `listen_addr = ":8485"`
4. Update firewall rules for new port

## 🆘 Getting Help

### Self-Service Options

1. **Run diagnostics:** `clipsync doctor`
2. **Check documentation:** [GitHub Wiki](https://github.com/lancekrogers/clipsync/wiki)
3. **Search issues:** [GitHub Issues](https://github.com/lancekrogers/clipsync/issues)

### Community Support

1. **GitHub Discussions:** Ask questions and share tips
2. **Discord Server:** Real-time community help
3. **Reddit:** r/clipsync community

### Professional Support

1. **Bug Reports:** [Create an Issue](https://github.com/lancekrogers/clipsync/issues/new)
2. **Feature Requests:** [Request Feature](https://github.com/lancekrogers/clipsync/issues/new)
3. **Security Issues:** Email security@clipsync.dev

### Information to Include

When asking for help, please include:

```bash
# System information
clipsync version
uname -a                        # System info
clipsync config show            # Configuration (redact keys)
clipsync status                 # Status
clipsync logs --tail 50         # Recent logs
```

**Template for Bug Reports:**
```markdown
## Problem Description
Brief description of the issue

## Steps to Reproduce
1. Start ClipSync
2. Copy text on Device A
3. Expected: text appears on Device B
4. Actual: nothing happens

## Environment
- OS: macOS 14.0 / Ubuntu 22.04
- ClipSync version: 1.0.0
- Installation: Homebrew / manual
- Network: Same WiFi / Different subnets

## Logs
```
(paste relevant log entries)
```

## Configuration
```toml
(paste config file with keys redacted)
```
```

---

**Remember:** Most ClipSync issues are related to network connectivity, authentication, or permissions. The built-in `clipsync doctor` command can diagnose and fix many common problems automatically!