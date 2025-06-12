# ClipSync Homebrew Formula

This directory contains Homebrew formulas for installing ClipSync.

## Installation Options

### Option 1: Install from Source (Recommended)

```bash
# Add the tap
brew tap lancekrogers/clipsync

# Install from source
brew install --HEAD lancekrogers/clipsync/clipsync-source

# Or with options
brew install --HEAD --with-debug lancekrogers/clipsync/clipsync-source
```

### Option 2: Install Pre-built Binary (Coming Soon)

```bash
# Once binaries are available
brew install lancekrogers/clipsync/clipsync
```

## Building from Source Locally

If you want to test the formula locally:

```bash
# Install directly from the formula file
brew install --build-from-source ./homebrew/clipsync-source.rb

# Or with debug symbols
brew install --build-from-source --with-debug ./homebrew/clipsync-source.rb
```

## Dependencies

The formula will automatically install:
- Rust toolchain (build-time)
- OpenSSL 3.x
- zstd compression library
- Platform-specific clipboard libraries

## Post-Installation

After installation:

```bash
# Start the service
brew services start clipsync-source

# Check status
clipsync status

# View configuration
clipsync config show

# Initialize SSH keys
clipsync auth init
```

## Updating the Formula

When releasing a new version:

1. Update the version number
2. Update the source tarball URL
3. Calculate SHA256: `shasum -a 256 clipsync-v0.1.0.tar.gz`
4. Update the sha256 field

## Troubleshooting

If the build fails:

```bash
# Check OpenSSL installation
brew list openssl@3

# Reinstall dependencies
brew reinstall openssl@3 zstd

# Build with verbose output
brew install --verbose --debug ./homebrew/clipsync-source.rb
```