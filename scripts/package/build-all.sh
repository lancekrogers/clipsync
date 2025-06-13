#!/usr/bin/env bash

set -euo pipefail

# Script to build packages for all supported platforms
# This orchestrates the platform-specific build scripts

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUILD_DIR="$PROJECT_ROOT/build"
DIST_DIR="$PROJECT_ROOT/dist"

# Configuration
VERSION=$(grep '^version' "$PROJECT_ROOT/Cargo.toml" | head -1 | cut -d'"' -f2)
CURRENT_OS=$(uname -s)

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
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

log_section() {
    echo -e "\n${BLUE}═══════════════════════════════════════${NC}"
    echo -e "${BLUE}║${NC} $1"
    echo -e "${BLUE}═══════════════════════════════════════${NC}\n"
}

clean_build() {
    log_info "Cleaning previous builds..."
    rm -rf "$BUILD_DIR"
    rm -rf "$DIST_DIR"
    mkdir -p "$BUILD_DIR"
    mkdir -p "$DIST_DIR"
}

build_current_platform() {
    case "$CURRENT_OS" in
        Darwin)
            log_section "Building macOS packages"
            bash "$SCRIPT_DIR/build-macos.sh"
            ;;
        Linux)
            log_section "Building Linux packages"
            bash "$SCRIPT_DIR/build-linux.sh"
            ;;
        *)
            log_error "Unsupported platform: $CURRENT_OS"
            exit 1
            ;;
    esac
}

build_cross_platform() {
    log_warn "Cross-platform builds require proper toolchain setup"
    
    # Check for cross-compilation tools
    if command -v cross &> /dev/null; then
        log_info "Found 'cross' tool for cross-compilation"
        
        log_section "Building for Linux targets"
        
        # Build for Linux x86_64
        if [ "$CURRENT_OS" != "Linux" ] || [ "$(uname -m)" != "x86_64" ]; then
            log_info "Cross-compiling for Linux x86_64..."
            cross build --release --target x86_64-unknown-linux-gnu
        fi
        
        # Build for Linux ARM64
        if [ "$CURRENT_OS" != "Linux" ] || [ "$(uname -m)" != "aarch64" ]; then
            log_info "Cross-compiling for Linux aarch64..."
            cross build --release --target aarch64-unknown-linux-gnu
        fi
        
        if [ "$CURRENT_OS" = "Darwin" ]; then
            log_section "Building for additional macOS targets"
            
            # Build for both Intel and Apple Silicon
            log_info "Building for x86_64-apple-darwin..."
            cargo build --release --target x86_64-apple-darwin
            
            log_info "Building for aarch64-apple-darwin..."
            cargo build --release --target aarch64-apple-darwin
        fi
    else
        log_warn "'cross' tool not found. Install with: cargo install cross"
        log_warn "Skipping cross-platform builds"
    fi
}

collect_artifacts() {
    log_section "Collecting build artifacts"
    
    # Find and copy all packages to dist directory
    find "$BUILD_DIR" -type f \( \
        -name "*.pkg" -o \
        -name "*.dmg" -o \
        -name "*.deb" -o \
        -name "*.rpm" -o \
        -name "*.tar.gz" -o \
        -name "*.AppImage" \
    \) -exec cp {} "$DIST_DIR/" \;
    
    # Generate checksums
    cd "$DIST_DIR"
    if ls *.* &> /dev/null; then
        log_info "Generating checksums..."
        shasum -a 256 * > SHA256SUMS
        
        # Create release notes
        cat > RELEASE_NOTES.md <<EOF
# ClipSync v$VERSION Release

## Packages

### macOS
- \`ClipSync-$VERSION-*.pkg\` - macOS installer package
- \`ClipSync-$VERSION-*.dmg\` - macOS disk image

### Linux
- \`clipsync-$VERSION-*.tar.gz\` - Generic Linux tarball
- \`clipsync_$VERSION_*.deb\` - Debian/Ubuntu package  
- \`clipsync-$VERSION-*.rpm\` - Fedora/RHEL package
- \`ClipSync-$VERSION-*.AppImage\` - Universal Linux AppImage

## Installation

### macOS
\`\`\`bash
# Using Homebrew
brew install clipsync

# Or download and open the .pkg file
\`\`\`

### Linux
\`\`\`bash
# Universal installer
curl -fsSL https://raw.githubusercontent.com/lancekrogers/clipsync/main/scripts/install.sh | bash

# Debian/Ubuntu
sudo dpkg -i clipsync_$VERSION_amd64.deb

# Fedora/RHEL
sudo rpm -i clipsync-$VERSION-1.x86_64.rpm

# Arch Linux (AUR)
yay -S clipsync
\`\`\`

## Checksums
See \`SHA256SUMS\` file for package checksums.
EOF
    fi
    
    log_info "Build artifacts collected in: $DIST_DIR"
    ls -la "$DIST_DIR"
}

print_summary() {
    log_section "Build Summary"
    
    echo "ClipSync v$VERSION - Build Complete!"
    echo
    echo "Packages created:"
    
    if [ -d "$DIST_DIR" ]; then
        cd "$DIST_DIR"
        for file in *; do
            if [ -f "$file" ] && [ "$file" != "SHA256SUMS" ] && [ "$file" != "RELEASE_NOTES.md" ]; then
                size=$(ls -lh "$file" | awk '{print $5}')
                echo "  - $file ($size)"
            fi
        done
    fi
    
    echo
    echo "Next steps:"
    echo "1. Test packages on target platforms"
    echo "2. Sign packages with appropriate certificates"
    echo "3. Upload to GitHub releases or package repositories"
    echo "4. Update download links in install scripts and documentation"
}

show_usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Build packages for ClipSync across all supported platforms.

Options:
    -c, --current     Build only for current platform
    -x, --cross       Include cross-platform builds
    -h, --help        Show this help message
    
Examples:
    $0                # Build for current platform only
    $0 --cross        # Build for all platforms (requires cross-compilation setup)

EOF
}

main() {
    local cross_compile=false
    
    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            -c|--current)
                cross_compile=false
                shift
                ;;
            -x|--cross)
                cross_compile=true
                shift
                ;;
            -h|--help)
                show_usage
                exit 0
                ;;
            *)
                log_error "Unknown option: $1"
                show_usage
                exit 1
                ;;
        esac
    done
    
    log_section "ClipSync Package Builder v1.0"
    log_info "Building packages for ClipSync v$VERSION"
    log_info "Current platform: $CURRENT_OS $(uname -m)"
    
    # Clean previous builds
    clean_build
    
    # Build for current platform
    build_current_platform
    
    # Optionally build for other platforms
    if [ "$cross_compile" = true ]; then
        build_cross_platform
    fi
    
    # Collect all artifacts
    collect_artifacts
    
    # Print summary
    print_summary
}

# Make scripts executable
chmod +x "$SCRIPT_DIR"/*.sh

# Run main
main "$@"