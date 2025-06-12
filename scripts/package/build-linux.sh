#!/usr/bin/env bash

set -euo pipefail

# Script to build Linux packages for ClipSync
# Creates .deb, .rpm, and .tar.gz packages

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PKG_DIR="$PROJECT_ROOT/pkg"
BUILD_DIR="$PROJECT_ROOT/build"
TARGET_DIR="$PROJECT_ROOT/target"

# Configuration
APP_NAME="clipsync"
VERSION=$(grep '^version' "$PROJECT_ROOT/Cargo.toml" | head -1 | cut -d'"' -f2)
ARCH=$(uname -m)
DEBIAN_ARCH="$ARCH"

# Convert architecture names
case "$ARCH" in
    x86_64)     DEBIAN_ARCH="amd64"; RPM_ARCH="x86_64";;
    aarch64)    DEBIAN_ARCH="arm64"; RPM_ARCH="aarch64";;
    *)          log_error "Unsupported architecture: $ARCH"; exit 1;;
esac

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
    local deps=("cargo" "tar" "gzip")
    for dep in "${deps[@]}"; do
        if ! command -v "$dep" &> /dev/null; then
            log_error "$dep is required but not installed"
            exit 1
        fi
    done
    
    # Check for package building tools
    if ! command -v dpkg-deb &> /dev/null; then
        log_warn "dpkg-deb not found. .deb package creation will be skipped."
        log_warn "Install with: apt-get install dpkg"
    fi
    
    if ! command -v rpmbuild &> /dev/null; then
        log_warn "rpmbuild not found. .rpm package creation will be skipped."
        log_warn "Install with: yum install rpm-build or apt-get install rpm"
    fi
}

build_binary() {
    log_info "Building ClipSync binary for Linux ($ARCH)..."
    cd "$PROJECT_ROOT"
    
    # Build release binary
    cargo build --release
    
    # Strip binary to reduce size
    strip "$TARGET_DIR/release/$APP_NAME"
}

create_tarball() {
    log_info "Creating tarball package..."
    
    TARBALL_DIR="$BUILD_DIR/tarball/$APP_NAME-$VERSION"
    rm -rf "$BUILD_DIR/tarball"
    mkdir -p "$TARBALL_DIR/bin"
    mkdir -p "$TARBALL_DIR/share/systemd/user"
    mkdir -p "$TARBALL_DIR/share/doc/$APP_NAME"
    
    # Copy files
    cp "$TARGET_DIR/release/$APP_NAME" "$TARBALL_DIR/bin/"
    cp "$PROJECT_ROOT/scripts/clipsync.service" "$TARBALL_DIR/share/systemd/user/"
    cp "$PROJECT_ROOT/README.md" "$TARBALL_DIR/share/doc/$APP_NAME/" 2>/dev/null || true
    cp "$PROJECT_ROOT/LICENSE"* "$TARBALL_DIR/share/doc/$APP_NAME/" 2>/dev/null || true
    
    # Create install script
    cat > "$TARBALL_DIR/install.sh" <<'EOF'
#!/bin/bash
set -e

INSTALL_PREFIX="${PREFIX:-/usr/local}"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"

echo "Installing ClipSync to $INSTALL_PREFIX..."

# Install binary
install -Dm755 bin/clipsync "$INSTALL_PREFIX/bin/clipsync"

# Install systemd service
mkdir -p "$SYSTEMD_USER_DIR"
install -Dm644 share/systemd/user/clipsync.service "$SYSTEMD_USER_DIR/clipsync.service"

# Install documentation
install -Dm644 share/doc/clipsync/* "$INSTALL_PREFIX/share/doc/clipsync/" 2>/dev/null || true

# Reload systemd
systemctl --user daemon-reload

echo "Installation complete!"
echo "To start ClipSync: systemctl --user start clipsync"
echo "To enable at boot: systemctl --user enable clipsync"
EOF
    chmod 755 "$TARBALL_DIR/install.sh"
    
    # Create tarball
    cd "$BUILD_DIR/tarball"
    tar -czf "$BUILD_DIR/$APP_NAME-$VERSION-$ARCH-linux.tar.gz" "$APP_NAME-$VERSION"
    
    log_info "Tarball created: $BUILD_DIR/$APP_NAME-$VERSION-$ARCH-linux.tar.gz"
}

build_deb() {
    if ! command -v dpkg-deb &> /dev/null; then
        log_warn "Skipping .deb package creation (dpkg-deb not found)"
        return
    fi
    
    log_info "Building .deb package..."
    
    DEB_DIR="$BUILD_DIR/deb/$APP_NAME-$VERSION"
    rm -rf "$BUILD_DIR/deb"
    mkdir -p "$DEB_DIR/DEBIAN"
    mkdir -p "$DEB_DIR/usr/bin"
    mkdir -p "$DEB_DIR/usr/share/doc/$APP_NAME"
    mkdir -p "$DEB_DIR/usr/share/$APP_NAME"
    
    # Copy binary
    cp "$TARGET_DIR/release/$APP_NAME" "$DEB_DIR/usr/bin/"
    chmod 755 "$DEB_DIR/usr/bin/$APP_NAME"
    
    # Copy service file
    cp "$PKG_DIR/debian/clipsync.service" "$DEB_DIR/usr/share/$APP_NAME/"
    
    # Copy documentation
    cp "$PROJECT_ROOT/README.md" "$DEB_DIR/usr/share/doc/$APP_NAME/" 2>/dev/null || true
    cp "$PROJECT_ROOT/LICENSE"* "$DEB_DIR/usr/share/doc/$APP_NAME/" 2>/dev/null || true
    
    # Create control file with correct architecture
    sed "s/Architecture: amd64/Architecture: $DEBIAN_ARCH/g" "$PKG_DIR/debian/control" > "$DEB_DIR/DEBIAN/control"
    
    # Copy maintainer scripts
    cp "$PKG_DIR/debian/postinst" "$DEB_DIR/DEBIAN/"
    cp "$PKG_DIR/debian/prerm" "$DEB_DIR/DEBIAN/"
    chmod 755 "$DEB_DIR/DEBIAN/postinst"
    chmod 755 "$DEB_DIR/DEBIAN/prerm"
    
    # Calculate installed size
    INSTALLED_SIZE=$(du -sk "$DEB_DIR" | cut -f1)
    echo "Installed-Size: $INSTALLED_SIZE" >> "$DEB_DIR/DEBIAN/control"
    
    # Build package
    dpkg-deb --build "$DEB_DIR" "$BUILD_DIR/${APP_NAME}_${VERSION}_${DEBIAN_ARCH}.deb"
    
    log_info "Debian package created: $BUILD_DIR/${APP_NAME}_${VERSION}_${DEBIAN_ARCH}.deb"
}

build_rpm() {
    if ! command -v rpmbuild &> /dev/null; then
        log_warn "Skipping .rpm package creation (rpmbuild not found)"
        return
    fi
    
    log_info "Building .rpm package..."
    
    # Setup RPM build tree
    RPM_BUILD="$BUILD_DIR/rpmbuild"
    rm -rf "$RPM_BUILD"
    mkdir -p "$RPM_BUILD"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
    
    # Create source tarball for RPM
    SRC_DIR="$RPM_BUILD/SOURCES/$APP_NAME-$VERSION"
    mkdir -p "$SRC_DIR"
    cp "$TARGET_DIR/release/$APP_NAME" "$SRC_DIR/"
    cp -r "$PROJECT_ROOT/scripts" "$SRC_DIR/"
    cp "$PROJECT_ROOT/README.md" "$SRC_DIR/" 2>/dev/null || true
    cp "$PROJECT_ROOT/LICENSE"* "$SRC_DIR/" 2>/dev/null || true
    
    cd "$RPM_BUILD/SOURCES"
    tar -czf "$APP_NAME-$VERSION.tar.gz" "$APP_NAME-$VERSION"
    
    # Copy and update spec file
    cp "$PKG_DIR/rpm/clipsync.spec" "$RPM_BUILD/SPECS/"
    
    # Build RPM
    rpmbuild -bb \
        --define "_topdir $RPM_BUILD" \
        --define "_arch $RPM_ARCH" \
        --target "$RPM_ARCH" \
        "$RPM_BUILD/SPECS/clipsync.spec"
    
    # Copy built RPM to build directory
    find "$RPM_BUILD/RPMS" -name "*.rpm" -exec cp {} "$BUILD_DIR/" \;
    
    log_info "RPM package created: $BUILD_DIR/${APP_NAME}-${VERSION}-1.${RPM_ARCH}.rpm"
}

build_appimage() {
    log_info "Building AppImage..."
    
    # Check if appimagetool is available
    if ! command -v appimagetool &> /dev/null; then
        log_warn "appimagetool not found. Downloading..."
        APPIMAGE_TOOL="$BUILD_DIR/appimagetool"
        curl -L "https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-$ARCH.AppImage" -o "$APPIMAGE_TOOL"
        chmod +x "$APPIMAGE_TOOL"
    else
        APPIMAGE_TOOL="appimagetool"
    fi
    
    # Create AppDir structure
    APPDIR="$BUILD_DIR/ClipSync.AppDir"
    rm -rf "$APPDIR"
    mkdir -p "$APPDIR/usr/bin"
    mkdir -p "$APPDIR/usr/share/applications"
    mkdir -p "$APPDIR/usr/share/icons/hicolor/256x256/apps"
    
    # Copy binary
    cp "$TARGET_DIR/release/$APP_NAME" "$APPDIR/usr/bin/"
    
    # Create desktop file
    cat > "$APPDIR/usr/share/applications/clipsync.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=ClipSync
Comment=Cross-platform clipboard synchronization
Exec=clipsync
Icon=clipsync
Categories=Utility;System;
Terminal=false
EOF
    
    # Create AppRun script
    cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/bash
SELF=$(readlink -f "$0")
HERE="${SELF%/*}"
export PATH="${HERE}/usr/bin:${PATH}"
exec "${HERE}/usr/bin/clipsync" "$@"
EOF
    chmod +x "$APPDIR/AppRun"
    
    # Create a simple icon (you should replace this with actual icon)
    echo "CS" > "$APPDIR/usr/share/icons/hicolor/256x256/apps/clipsync.png"
    
    # Build AppImage
    "$APPIMAGE_TOOL" "$APPDIR" "$BUILD_DIR/ClipSync-$VERSION-$ARCH.AppImage" 2>/dev/null || true
    
    if [ -f "$BUILD_DIR/ClipSync-$VERSION-$ARCH.AppImage" ]; then
        log_info "AppImage created: $BUILD_DIR/ClipSync-$VERSION-$ARCH.AppImage"
    else
        log_warn "AppImage creation failed"
    fi
}

main() {
    log_info "Building Linux packages for ClipSync v$VERSION"
    
    # Check dependencies
    check_dependencies
    
    # Build binary
    build_binary
    
    # Create packages
    create_tarball
    build_deb
    build_rpm
    build_appimage
    
    log_info "Build complete!"
    log_info "Packages created in: $BUILD_DIR"
    ls -la "$BUILD_DIR"/*.{tar.gz,deb,rpm,AppImage} 2>/dev/null || true
}

# Run main
main "$@"