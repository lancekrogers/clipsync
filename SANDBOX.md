# ClipSync Development Sandboxing Guide

This guide provides multiple layers of sandboxing to prevent the clipboard sync tool from interfering with your system during development.

## Quick Start

Choose one of these sandboxing methods based on your needs:

### 1. **Lightweight: Safe Development Wrapper** (Recommended for quick testing)
```bash
# Run your development binary safely
./scripts/safe-dev.sh cargo run

# Or run the compiled binary
./scripts/safe-dev.sh ./target/debug/clipsync
```

### 2. **Medium: Dedicated Test User** (Good for integration testing)
```bash
# One-time setup
./scripts/setup-sandbox-user.sh

# Switch to test user
su - clipsync-test
cd ~/clipsync-dev
cargo build && cargo run
```

### 3. **Heavy: Systemd-nspawn Container** (Best isolation)
```bash
# One-time setup
sudo ./scripts/setup-nspawn-container.sh

# Enter container for development
clipsync-container shell
cd ~/clipsync
cargo build && cargo run
```

### 4. **Production: AppArmor Profile** (For installed binary)
```bash
# Install AppArmor profile
sudo ./scripts/install-apparmor.sh

# The installed binary at /usr/local/bin/clipsync will now be restricted
```

## Sandboxing Features Comparison

| Method | Isolation Level | Setup Complexity | Performance Impact | Use Case |
|--------|----------------|------------------|-------------------|-----------|
| Safe Dev Wrapper | Medium | None | Low | Quick development/testing |
| Test User | Medium | Low | None | Integration testing |
| Nspawn Container | High | Medium | Low | Full system testing |
| AppArmor | Medium | Low | None | Production deployment |

## What Each Method Protects

### Safe Development Wrapper (`safe-dev.sh`)
- ✅ Isolated HOME directory (temp)
- ✅ No access to SSH keys, GPG keys
- ✅ Memory and CPU limits
- ✅ Read-only system directories
- ✅ No access to sensitive env variables
- ✅ Optional firejail sandboxing

### Test User
- ✅ Separate user account
- ✅ No sudo access
- ✅ Isolated home directory
- ✅ Can't affect main user's files
- ❌ Still shares system resources

### Systemd-nspawn Container
- ✅ Complete filesystem isolation
- ✅ Separate process namespace
- ✅ Network isolation (optional)
- ✅ Can't affect host system
- ✅ Easy to reset/recreate

### AppArmor Profile
- ✅ Prevents access to sensitive files
- ✅ Blocks execution of other programs
- ✅ Restricts network access (configurable)
- ✅ Allows only necessary clipboard operations
- ✅ Enforced by kernel

## Development Workflow

### For Regular Development:
```bash
# Always use the wrapper during development
./scripts/safe-dev.sh cargo run -- start

# Run tests safely
./scripts/safe-dev.sh cargo test

# Build safely
./scripts/safe-dev.sh cargo build --release
```

### For Testing Clipboard Sync:
```bash
# Terminal 1 - Start first instance
./scripts/safe-dev.sh cargo run -- start

# Terminal 2 - Start second instance on different port
./scripts/safe-dev.sh cargo run -- --port 45447 start

# Test clipboard operations safely
```

### For Debugging Issues:
```bash
# Use strace within sandbox
./scripts/safe-dev.sh strace -f cargo run 2>&1 | tee trace.log

# Check what files are being accessed
grep -E "open|openat" trace.log | grep -v ENOENT
```

## Best Practices

1. **Always use sandboxing during development** - Never run the development binary directly
2. **Test in container before system-wide testing** - Use nspawn for integration tests
3. **Review file access** - Check what files your tool is trying to access
4. **Minimal permissions** - Only grant what's absolutely necessary
5. **Regular audits** - Periodically review your code for unsafe operations

## Monitoring and Debugging

### Check if sandboxing is working:
```bash
# In safe-dev wrapper, this should show temp directory
./scripts/safe-dev.sh ./target/debug/clipsync -- bash -c 'echo $HOME'

# Should not be able to read your SSH keys
./scripts/safe-dev.sh ./target/debug/clipsync -- cat ~/.ssh/id_rsa
# Expected: Permission denied or No such file
```

### Monitor file access:
```bash
# Install monitoring tools
sudo pacman -S audit

# Monitor what clipsync accesses
sudo auditctl -w /etc/passwd -p r -k clipsync_passwd
sudo auditctl -w /home -p w -k clipsync_write

# Check audit log
sudo ausearch -k clipsync_passwd
```

## Emergency Recovery

If something goes wrong:

1. **Kill all clipsync processes**:
   ```bash
   pkill -f clipsync
   ```

2. **Remove test user** (if created):
   ```bash
   sudo userdel -r clipsync-test
   ```

3. **Remove container** (if created):
   ```bash
   sudo rm -rf /var/lib/machines/clipsync-dev
   ```

4. **Disable AppArmor profile**:
   ```bash
   sudo aa-disable /usr/local/bin/clipsync
   ```

## Security Checklist

Before running clipsync, ensure:
- [ ] Using one of the sandboxing methods
- [ ] Not running as root or with sudo
- [ ] Config files don't contain sensitive data
- [ ] Not accessing files outside of designated directories
- [ ] Network connections are to expected peers only

## Additional Resources

- [Firejail Documentation](https://firejail.wordpress.com/)
- [systemd-nspawn Documentation](https://www.freedesktop.org/software/systemd/man/systemd-nspawn.html)
- [AppArmor Documentation](https://wiki.archlinux.org/title/AppArmor)

Remember: **When in doubt, use more sandboxing, not less!**