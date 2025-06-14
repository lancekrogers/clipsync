# ClipSync Pre-Release Checklist

## Before Making Public

### Code Quality
- [x] All tests pass (`make test`)
- [x] No compilation warnings
- [x] Dependencies updated to latest versions
- [ ] Security audit (`cargo audit`)
- [ ] Code formatted (`cargo fmt`)
- [ ] Linter passes (`cargo clippy`)

### Documentation
- [x] README.md is complete and accurate
- [x] INSTALL.md provides clear installation instructions
- [ ] API documentation generated (`cargo doc`)
- [ ] License file added (MIT or Apache-2.0)
- [ ] CONTRIBUTING.md for contributors

### Testing on macOS
1. Build release binary:
   ```bash
   make release
   ```

2. Test basic functionality:
   ```bash
   ./test_install.sh
   ```

3. Test actual installation:
   ```bash
   # Test without actually installing
   make -n install
   
   # If everything looks good
   make install
   ```

4. Test clipboard sync:
   - Copy some text
   - Run `clipsync paste`
   - Check history with `clipsync history`

5. Test service management:
   ```bash
   clipsync status
   clipsync stop
   clipsync start
   ```

### Testing on Arch Linux
1. Copy files to Arch system:
   ```bash
   rsync -av --exclude target/ ./ user@arch-system:~/clipsync/
   ```

2. On Arch system, run:
   ```bash
   ./test_arch_install.sh
   ```

3. Test installation:
   ```bash
   sudo make install
   systemctl --user status clipsync
   ```

4. Test clipboard operations with both X11 and Wayland

### Cross-Platform Sync Testing
1. Install on both macOS and Arch Linux
2. Ensure both are on same network
3. Start services on both systems
4. Copy text on one system
5. Verify it appears on the other
6. Test with different content types (text, images if supported)

### Security Review
- [ ] SSH key generation works correctly
- [ ] Encryption is enabled by default
- [ ] No sensitive data in logs
- [ ] Config files have proper permissions
- [ ] No hardcoded secrets or keys

### Repository Preparation
- [ ] `.gitignore` includes all necessary files
- [ ] No binary files in repository
- [ ] CI/CD configuration (GitHub Actions)
- [ ] Create initial release tag
- [ ] Write release notes

### Final Steps
1. Remove debug/test code
2. Update version in Cargo.toml
3. Create GitHub repository
4. Push code
5. Create initial release
6. Test installation from GitHub

## Post-Release
- [ ] Monitor issues for installation problems
- [ ] Create packages for package managers (Homebrew, AUR)
- [ ] Set up documentation site
- [ ] Create demo video/GIF