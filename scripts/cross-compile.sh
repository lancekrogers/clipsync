#!/bin/bash
set -e

# ClipSync Cross-Compilation Setup Script
# Sets up the environment for cross-compiling ClipSync to different platforms

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Print colored output
print_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if running on supported OS
check_os() {
    case "$(uname -s)" in
        Linux*)     OS=Linux;;
        Darwin*)    OS=Mac;;
        *)          
            print_error "Unsupported operating system: $(uname -s)"
            exit 1
            ;;
    esac
    print_info "Detected OS: $OS"
}

# Install cross compilation tool
install_cross() {
    if command -v cross &> /dev/null; then
        print_info "cross is already installed"
        return
    fi

    print_info "Installing cross..."
    cargo install cross --git https://github.com/cross-rs/cross
    print_success "cross installed successfully"
}

# Install Docker (required for cross on some platforms)
check_docker() {
    if ! command -v docker &> /dev/null; then
        print_warning "Docker is not installed. cross requires Docker for Linux cross-compilation."
        echo "Please install Docker from https://docs.docker.com/get-docker/"
        return 1
    fi

    if ! docker info &> /dev/null; then
        print_warning "Docker is not running. Please start Docker."
        return 1
    fi

    print_info "Docker is installed and running"
    return 0
}

# Setup Rust targets
setup_rust_targets() {
    print_info "Setting up Rust compilation targets..."

    # Define targets
    TARGETS=(
        "x86_64-unknown-linux-gnu"
        "aarch64-unknown-linux-gnu"
        "x86_64-apple-darwin"
        "aarch64-apple-darwin"
    )

    # Install targets
    for target in "${TARGETS[@]}"; do
        print_info "Adding target: $target"
        rustup target add "$target" || print_warning "Failed to add target: $target"
    done

    print_success "Rust targets configured"
}

# Setup Linux cross-compilation dependencies
setup_linux_cross() {
    if [ "$OS" != "Linux" ]; then
        return
    fi

    print_info "Setting up Linux cross-compilation dependencies..."

    # Install required packages
    sudo apt-get update
    sudo apt-get install -y \
        gcc-aarch64-linux-gnu \
        g++-aarch64-linux-gnu \
        libc6-dev-arm64-cross \
        pkg-config \
        libssl-dev

    print_success "Linux cross-compilation dependencies installed"
}

# Setup macOS cross-compilation
setup_macos_cross() {
    print_info "Setting up macOS cross-compilation..."

    if [ "$OS" = "Mac" ]; then
        # On macOS, we can compile for both architectures natively
        print_info "macOS supports native compilation for both x86_64 and aarch64"
    else
        print_warning "Cross-compiling to macOS from Linux requires osxcross"
        echo "See: https://github.com/tpoechtrager/osxcross"
        echo "Note: This requires macOS SDK which has licensing restrictions"
    fi
}

# Create cargo config for cross-compilation
create_cargo_config() {
    print_info "Creating cargo configuration for cross-compilation..."

    CARGO_CONFIG_DIR="$PROJECT_ROOT/.cargo"
    CARGO_CONFIG_FILE="$CARGO_CONFIG_DIR/config.toml"

    mkdir -p "$CARGO_CONFIG_DIR"

    cat > "$CARGO_CONFIG_FILE" << 'EOF'
# Cross-compilation configuration for ClipSync

[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
ar = "aarch64-linux-gnu-ar"

[target.x86_64-unknown-linux-gnu]
# Use system linker for native compilation

[target.x86_64-apple-darwin]
# macOS x86_64 configuration
# Note: Cross-compilation from Linux requires osxcross

[target.aarch64-apple-darwin]
# macOS ARM64 configuration
# Note: Cross-compilation from Linux requires osxcross

# Build settings
[build]
# Optimize for size in release builds
rustflags = ["-C", "link-arg=-s"]

[profile.release]
opt-level = "z"     # Optimize for size
lto = true          # Enable Link Time Optimization
codegen-units = 1   # Single codegen unit for better optimization
strip = true        # Strip symbols
panic = "abort"     # Smaller binary size

# Platform-specific environment variables
[env]
# OpenSSL configuration for cross-compilation
OPENSSL_STATIC = "1"
OPENSSL_LIB_DIR = "/usr/lib/x86_64-linux-gnu"
OPENSSL_INCLUDE_DIR = "/usr/include/openssl"
EOF

    print_success "Cargo configuration created"
}

# Build for a specific target
build_target() {
    local target=$1
    local use_cross=$2

    print_info "Building for target: $target"

    cd "$PROJECT_ROOT"

    if [ "$use_cross" = "true" ] && command -v cross &> /dev/null; then
        print_info "Using cross for compilation..."
        cross build --release --target "$target"
    else
        print_info "Using cargo for compilation..."
        cargo build --release --target "$target"
    fi

    # Check if build succeeded
    local binary_path="$PROJECT_ROOT/target/$target/release/clipsync"
    if [ -f "$binary_path" ]; then
        print_success "Build successful: $binary_path"
        ls -lh "$binary_path"
    else
        print_error "Build failed for target: $target"
        return 1
    fi
}

# Build all targets
build_all() {
    print_info "Building for all supported targets..."

    # Determine which targets to build based on OS
    if [ "$OS" = "Mac" ]; then
        # On macOS, build for both macOS architectures
        build_target "x86_64-apple-darwin" false
        build_target "aarch64-apple-darwin" false
        
        # Use cross for Linux targets (requires Docker)
        if check_docker; then
            build_target "x86_64-unknown-linux-gnu" true
            build_target "aarch64-unknown-linux-gnu" true
        fi
    else
        # On Linux, build for Linux targets natively
        build_target "x86_64-unknown-linux-gnu" false
        
        # Use cross for ARM Linux
        build_target "aarch64-unknown-linux-gnu" true
        
        # macOS targets require osxcross
        print_warning "Skipping macOS targets (requires osxcross setup)"
    fi

    print_success "All builds completed"
}

# Package binaries
package_binaries() {
    print_info "Packaging binaries..."

    local output_dir="$PROJECT_ROOT/release"
    mkdir -p "$output_dir"

    # Find all built binaries
    for target_dir in "$PROJECT_ROOT/target"/*/release; do
        if [ -f "$target_dir/clipsync" ]; then
            local target=$(basename "$(dirname "$target_dir")")
            local archive_name="clipsync-${target}.tar.gz"
            
            print_info "Packaging $target..."
            
            # Create archive
            (cd "$target_dir" && tar czf "$output_dir/$archive_name" clipsync)
            
            # Generate checksum
            (cd "$output_dir" && shasum -a 256 "$archive_name" > "${archive_name}.sha256")
            
            print_success "Created $archive_name"
        fi
    done

    print_success "All binaries packaged in: $output_dir"
    ls -lh "$output_dir"
}

# Show usage
usage() {
    echo "Usage: $0 [command]"
    echo ""
    echo "Commands:"
    echo "  setup     - Install dependencies and configure environment"
    echo "  build     - Build for all supported targets"
    echo "  package   - Package built binaries"
    echo "  all       - Run setup, build, and package"
    echo "  help      - Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0 setup          # Setup cross-compilation environment"
    echo "  $0 build          # Build for all targets"
    echo "  $0 all            # Complete cross-compilation workflow"
}

# Main function
main() {
    case "${1:-all}" in
        setup)
            check_os
            install_cross
            check_docker
            setup_rust_targets
            setup_linux_cross
            setup_macos_cross
            create_cargo_config
            print_success "Cross-compilation setup complete"
            ;;
        build)
            check_os
            build_all
            ;;
        package)
            package_binaries
            ;;
        all)
            check_os
            install_cross
            check_docker
            setup_rust_targets
            setup_linux_cross
            setup_macos_cross
            create_cargo_config
            build_all
            package_binaries
            ;;
        help)
            usage
            ;;
        *)
            print_error "Unknown command: $1"
            usage
            exit 1
            ;;
    esac
}

# Run main function
main "$@"