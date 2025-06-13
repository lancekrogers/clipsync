# Agent 1: Packaging & Installation Specialist

## Your Mission
You are responsible for creating professional-grade installers and packages for ClipSync across all supported platforms. Your work will make it easy for users to install and run ClipSync with minimal effort.

## Context
- ClipSync is a Rust application for cross-platform clipboard synchronization
- The binary is built with `cargo build --release`
- Target platforms: macOS (Intel & Apple Silicon) and Linux (x86_64)
- The application needs to run as a background service

## Status: COMPLETED ✅

All packaging infrastructure has been successfully created:

### Completed Tasks
1. ✅ Created complete pkg directory structure
2. ✅ Created macOS package files (Info.plist, distribution.xml, resources)
3. ✅ Created Debian package files (control, postinst, prerm, service)
4. ✅ Created RPM spec file
5. ✅ Created Homebrew formula
6. ✅ Created AUR PKGBUILD
7. ✅ Created universal install.sh script
8. ✅ Created platform-specific build scripts

### Key Deliverables Created

#### 1. Universal Installer (`scripts/install.sh`)
- Auto-detects OS and architecture
- Downloads appropriate binary from GitHub releases
- Installs service files for macOS (launchd) and Linux (systemd)
- Creates necessary directories with proper permissions

#### 2. Platform-Specific Packages

**macOS:**
- Full `.pkg` installer with welcome/conclusion screens
- `distribution.xml` for customized installation flow
- Code signing support (with documentation)
- DMG creation script
- Universal binary support (Intel + Apple Silicon)

**Linux:**
- Debian `.deb` package with full systemd integration
- RPM `.spec` file for Fedora/RHEL
- AUR PKGBUILD for Arch Linux
- Generic tarball with install script
- AppImage support

#### 3. Build Automation Scripts
- `build-macos.sh` - Builds pkg and dmg files
- `build-linux.sh` - Builds deb, rpm, tarball, and AppImage
- `build-all.sh` - Orchestrates all platform builds

#### 4. Package Management Integration
- Homebrew formula ready for tap submission
- Service management via `brew services`
- Systemd user service (not requiring root)
- Launchd agent for macOS auto-start

### Usage Instructions

To build packages:
```bash
# Build for current platform
./scripts/package/build-all.sh

# Build for all platforms (requires cross-compilation tools)
./scripts/package/build-all.sh --cross
```

To install using universal installer:
```bash
curl -fsSL https://raw.githubusercontent.com/yourusername/clipsync/main/scripts/install.sh | bash
```

## Your Tasks

### 1. Platform-Specific Packages
Create installation packages for each platform:

#### macOS
- Create a `.pkg` installer using `pkgbuild` and `productbuild`
- Include proper `Info.plist` and bundle structure
- Set up code signing (document process even if certs not available)
- Create a DMG with a nice background image and app icon
- Support both Intel and Apple Silicon

#### Linux
- Create `.deb` package for Debian/Ubuntu
  - Include proper control file
  - Set up post-install scripts
  - Handle systemd service installation
- Create `.rpm` package for Fedora/RHEL
  - Write proper spec file
  - Include service management
- Create AUR PKGBUILD for Arch Linux
- Make packages architecture-aware (amd64, arm64)

### 2. Service Management

#### macOS launchd
- Create `com.clipsync.plist` for launchd
- Set up auto-start on login
- Handle service start/stop/restart
- Put in `~/Library/LaunchAgents/`

#### Linux systemd
- Create `clipsync.service` unit file
- Set up user service (not system-wide)
- Enable auto-start
- Handle service management commands

### 3. Installation Scripts
- Create universal `install.sh` script that:
  - Detects the OS and architecture
  - Downloads the appropriate binary
  - Installs to correct location (`/usr/local/bin` or similar)
  - Sets up service files
  - Creates necessary directories
  - Sets proper permissions

### 4. Homebrew Formula
- Create `clipsync.rb` formula
- Set up bottle (pre-compiled binary) support
- Include service management via `brew services`
- Test with `brew install --build-from-source`

### 5. Build Scripts
Create scripts to automate the build process:
- `scripts/package/build-macos.sh` - Builds macOS packages
- `scripts/package/build-linux.sh` - Builds Linux packages
- `scripts/package/build-all.sh` - Builds everything
- Include cross-compilation setup

## Important Considerations
- **Permissions**: Ensure proper file permissions (755 for executables, 644 for configs)
- **Paths**: Use standard paths for each platform
- **Dependencies**: Document any runtime dependencies
- **Architecture**: Support both x86_64 and ARM architectures
- **Signing**: Document code signing process for macOS
- **Service**: Ensure service files work with user permissions (not root)

## Directory Structure to Create
```
scripts/
  package/
    build-macos.sh
    build-linux.sh
    build-all.sh
  install.sh
pkg/
  macos/
    Info.plist
    distribution.xml
    Resources/
  debian/
    control
    postinst
    prerm
    clipsync.service
  rpm/
    clipsync.spec
  homebrew/
    clipsync.rb
  aur/
    PKGBUILD
```

## Deliverables
1. Working installers for all platforms
2. Installation scripts that work reliably
3. Service management that starts on boot
4. Clear build documentation
5. Tested packages on each platform

## Testing Your Work
- Test installation on fresh VMs
- Verify service auto-start
- Check uninstallation is clean
- Ensure upgrades work properly
- Test on different OS versions

Remember: The goal is to make installation as simple as possible for end users!