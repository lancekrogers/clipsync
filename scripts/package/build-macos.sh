#!/usr/bin/env bash

set -euo pipefail

# Script to build macOS packages for ClipSync
# Creates both .pkg installer and .dmg disk image

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PKG_DIR="$PROJECT_ROOT/pkg"
BUILD_DIR="$PROJECT_ROOT/build"
TARGET_DIR="$PROJECT_ROOT/target"

# Configuration
APP_NAME="ClipSync"
BUNDLE_ID="com.clipsync"
VERSION=$(grep '^version' "$PROJECT_ROOT/Cargo.toml" | head -1 | cut -d'"' -f2)
ARCH=$(uname -m)

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1" >&2
}

check_dependencies() {
    local deps=("cargo" "pkgbuild" "productbuild" "hdiutil" "create-dmg")
    for dep in "${deps[@]}"; do
        if ! command -v "$dep" &> /dev/null; then
            if [ "$dep" = "create-dmg" ]; then
                log_warn "create-dmg not found. DMG creation will use basic hdiutil instead."
                log_warn "Install with: brew install create-dmg"
            else
                log_error "$dep is required but not installed"
                exit 1
            fi
        fi
    done
}

build_binary() {
    log_info "Building ClipSync binary for macOS ($ARCH)..."
    cd "$PROJECT_ROOT"
    
    # Build for current architecture
    cargo build --release
    
    # Also build universal binary if on Apple Silicon
    if [ "$ARCH" = "arm64" ]; then
        log_info "Building universal binary..."
        cargo build --release --target x86_64-apple-darwin
        cargo build --release --target aarch64-apple-darwin
        
        # Create universal binary
        lipo -create \
            "$TARGET_DIR/x86_64-apple-darwin/release/clipsync" \
            "$TARGET_DIR/aarch64-apple-darwin/release/clipsync" \
            -output "$TARGET_DIR/release/clipsync-universal"
    fi
}

prepare_package_root() {
    log_info "Preparing package root..."
    
    # Clean and create build directory
    rm -rf "$BUILD_DIR/macos"
    mkdir -p "$BUILD_DIR/macos/root/usr/local/bin"
    mkdir -p "$BUILD_DIR/macos/root/Library/LaunchAgents"
    mkdir -p "$BUILD_DIR/macos/scripts"
    
    # Copy binary
    if [ -f "$TARGET_DIR/release/clipsync-universal" ]; then
        cp "$TARGET_DIR/release/clipsync-universal" "$BUILD_DIR/macos/root/usr/local/bin/clipsync"
    else
        cp "$TARGET_DIR/release/clipsync" "$BUILD_DIR/macos/root/usr/local/bin/clipsync"
    fi
    chmod 755 "$BUILD_DIR/macos/root/usr/local/bin/clipsync"
    
    # Copy launch agent plist
    cp "$PROJECT_ROOT/scripts/com.clipsync.plist" "$BUILD_DIR/macos/root/Library/LaunchAgents/"
    
    # Create postinstall script
    cat > "$BUILD_DIR/macos/scripts/postinstall" <<'EOF'
#!/bin/bash

# Post-installation script for ClipSync

# Get the current user
CURRENT_USER=$(stat -f "%Su" /dev/console)
USER_ID=$(id -u "$CURRENT_USER")

# Copy LaunchAgent to user directory
if [ "$CURRENT_USER" != "root" ]; then
    USER_HOME=$(eval echo ~"$CURRENT_USER")
    LAUNCH_AGENTS_DIR="$USER_HOME/Library/LaunchAgents"
    
    # Create LaunchAgents directory if it doesn't exist
    sudo -u "$CURRENT_USER" mkdir -p "$LAUNCH_AGENTS_DIR"
    
    # Copy plist file
    cp "/Library/LaunchAgents/com.clipsync.plist" "$LAUNCH_AGENTS_DIR/"
    chown "$CURRENT_USER:staff" "$LAUNCH_AGENTS_DIR/com.clipsync.plist"
    
    # Load the service
    sudo -u "$CURRENT_USER" launchctl load "$LAUNCH_AGENTS_DIR/com.clipsync.plist" 2>/dev/null || true
    
    # Remove system-wide plist
    rm -f "/Library/LaunchAgents/com.clipsync.plist"
fi

exit 0
EOF
    chmod 755 "$BUILD_DIR/macos/scripts/postinstall"
}

build_pkg() {
    log_info "Building .pkg installer..."
    
    # Build component package
    pkgbuild \
        --root "$BUILD_DIR/macos/root" \
        --identifier "$BUNDLE_ID" \
        --version "$VERSION" \
        --scripts "$BUILD_DIR/macos/scripts" \
        --ownership recommended \
        "$BUILD_DIR/macos/ClipSync-component.pkg"
    
    # Copy resources
    cp -r "$PKG_DIR/macos/Resources" "$BUILD_DIR/macos/"
    
    # Create distribution.xml with correct version
    sed "s/0.1.0/$VERSION/g" "$PKG_DIR/macos/distribution.xml" > "$BUILD_DIR/macos/distribution.xml"
    
    # Build product package
    productbuild \
        --distribution "$BUILD_DIR/macos/distribution.xml" \
        --resources "$BUILD_DIR/macos/Resources" \
        --package-path "$BUILD_DIR/macos" \
        "$BUILD_DIR/ClipSync-$VERSION-$ARCH.pkg"
    
    log_info "Package created: $BUILD_DIR/ClipSync-$VERSION-$ARCH.pkg"
}

build_dmg() {
    log_info "Building .dmg disk image..."
    
    # Prepare DMG contents
    DMG_DIR="$BUILD_DIR/macos/dmg"
    rm -rf "$DMG_DIR"
    mkdir -p "$DMG_DIR"
    
    # Copy installer
    cp "$BUILD_DIR/ClipSync-$VERSION-$ARCH.pkg" "$DMG_DIR/Install ClipSync.pkg"
    
    # Create README
    cat > "$DMG_DIR/README.txt" <<EOF
ClipSync $VERSION

To install ClipSync:
1. Double-click "Install ClipSync.pkg"
2. Follow the installation instructions
3. ClipSync will start automatically

For more information, visit:
https://github.com/lancekrogers/clipsync
EOF
    
    # Try to use create-dmg if available
    if command -v create-dmg &> /dev/null; then
        create-dmg \
            --volname "ClipSync $VERSION" \
            --window-size 600 400 \
            --icon-size 100 \
            --icon "Install ClipSync.pkg" 200 150 \
            --hide-extension "Install ClipSync.pkg" \
            --app-drop-link 400 150 \
            "$BUILD_DIR/ClipSync-$VERSION-$ARCH.dmg" \
            "$DMG_DIR"
    else
        # Fallback to basic hdiutil
        hdiutil create -volname "ClipSync $VERSION" \
            -srcfolder "$DMG_DIR" \
            -ov -format UDZO \
            "$BUILD_DIR/ClipSync-$VERSION-$ARCH.dmg"
    fi
    
    log_info "DMG created: $BUILD_DIR/ClipSync-$VERSION-$ARCH.dmg"
}

sign_package() {
    log_info "Code signing..."
    
    # Check if signing identity is available
    if security find-identity -v -p codesigning | grep -q "Developer ID"; then
        SIGN_IDENTITY=$(security find-identity -v -p codesigning | grep "Developer ID" | head -1 | awk '{print $2}')
        
        # Sign the binary
        codesign --force --deep --sign "$SIGN_IDENTITY" \
            --options runtime \
            --entitlements "$PKG_DIR/macos/entitlements.plist" \
            "$BUILD_DIR/macos/root/usr/local/bin/clipsync" 2>/dev/null || true
        
        # Sign the package
        productsign --sign "$SIGN_IDENTITY" \
            "$BUILD_DIR/ClipSync-$VERSION-$ARCH.pkg" \
            "$BUILD_DIR/ClipSync-$VERSION-$ARCH-signed.pkg" 2>/dev/null || true
        
        if [ -f "$BUILD_DIR/ClipSync-$VERSION-$ARCH-signed.pkg" ]; then
            mv "$BUILD_DIR/ClipSync-$VERSION-$ARCH-signed.pkg" "$BUILD_DIR/ClipSync-$VERSION-$ARCH.pkg"
            log_info "Package signed successfully"
        fi
    else
        log_warn "No Developer ID certificate found. Package will not be signed."
        log_warn "To sign packages, you need a valid Apple Developer ID certificate."
    fi
}

main() {
    log_info "Building macOS packages for ClipSync v$VERSION"
    
    # Check dependencies
    check_dependencies
    
    # Build binary
    build_binary
    
    # Prepare package
    prepare_package_root
    
    # Build installer package
    build_pkg
    
    # Sign if possible
    sign_package
    
    # Build DMG
    build_dmg
    
    log_info "Build complete!"
    log_info "Packages created in: $BUILD_DIR"
    ls -la "$BUILD_DIR"/*.{pkg,dmg} 2>/dev/null || true
}

# Run main
main "$@"