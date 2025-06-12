# Task 03: Build System Setup

## Objective
Create a comprehensive build system using Makefile and just for building, testing, and packaging ClipSync.

## Steps

1. **Create Makefile**
   ```makefile
   # ClipSync Makefile
   
   CARGO := cargo
   TARGET_DIR := target
   RELEASE_DIR := $(TARGET_DIR)/release
   
   # Platform detection
   UNAME_S := $(shell uname -s)
   UNAME_M := $(shell uname -m)
   
   ifeq ($(UNAME_S),Darwin)
       PLATFORM := macos
       ifeq ($(UNAME_M),arm64)
           TARGET := aarch64-apple-darwin
       else
           TARGET := x86_64-apple-darwin
       endif
   else ifeq ($(UNAME_S),Linux)
       PLATFORM := linux
       ifeq ($(UNAME_M),aarch64)
           TARGET := aarch64-unknown-linux-gnu
       else
           TARGET := x86_64-unknown-linux-gnu
       endif
   endif
   
   .PHONY: all build release test clean install uninstall fmt lint bench
   
   all: build
   
   build:
   	$(CARGO) build --target $(TARGET)
   
   release:
   	$(CARGO) build --release --target $(TARGET)
   
   test:
   	$(CARGO) test --all-features
   	$(CARGO) test --doc
   
   test-integration:
   	$(CARGO) test --test '*' -- --test-threads=1
   
   clean:
   	$(CARGO) clean
   	rm -rf dist/
   
   fmt:
   	$(CARGO) fmt -- --check
   	$(CARGO) clippy -- -D warnings
   
   fmt-fix:
   	$(CARGO) fmt
   	$(CARGO) clippy --fix --allow-staged
   
   lint:
   	$(CARGO) clippy -- -D warnings
   	$(CARGO) audit
   
   bench:
   	$(CARGO) bench
   
   install: release
   ifeq ($(PLATFORM),macos)
   	cp $(RELEASE_DIR)/clipsync /usr/local/bin/
   	cp dist/com.clipsync.plist ~/Library/LaunchAgents/
   	launchctl load ~/Library/LaunchAgents/com.clipsync.plist
   else
   	sudo cp $(RELEASE_DIR)/clipsync /usr/local/bin/
   	sudo cp dist/clipsync.service /etc/systemd/system/
   	sudo systemctl daemon-reload
   	sudo systemctl enable clipsync
   endif
   
   uninstall:
   ifeq ($(PLATFORM),macos)
   	launchctl unload ~/Library/LaunchAgents/com.clipsync.plist
   	rm -f ~/Library/LaunchAgents/com.clipsync.plist
   	rm -f /usr/local/bin/clipsync
   else
   	sudo systemctl stop clipsync
   	sudo systemctl disable clipsync
   	sudo rm -f /etc/systemd/system/clipsync.service
   	sudo rm -f /usr/local/bin/clipsync
   endif
   
   package: release
   	mkdir -p dist/$(PLATFORM)
   	cp $(RELEASE_DIR)/clipsync dist/$(PLATFORM)/
   ifeq ($(PLATFORM),macos)
   	cp scripts/com.clipsync.plist dist/
   	tar -czf dist/clipsync-$(PLATFORM)-$(UNAME_M).tar.gz -C dist/$(PLATFORM) .
   else
   	cp scripts/clipsync.service dist/
   	tar -czf dist/clipsync-$(PLATFORM)-$(UNAME_M).tar.gz -C dist/$(PLATFORM) .
   endif
   ```

2. **Create justfile**
   ```just
   # ClipSync build recipes
   
   default:
     @just --list
   
   # Build debug binary
   build:
     cargo build
   
   # Build release binary
   release:
     cargo build --release
   
   # Run all tests
   test:
     cargo test --all-features
     cargo test --doc
   
   # Run integration tests
   test-integration:
     cargo test --test '*' -- --test-threads=1
   
   # Run benchmarks
   bench:
     cargo bench
   
   # Format code
   fmt:
     cargo fmt
     cargo clippy --fix --allow-staged
   
   # Check formatting and lints
   check:
     cargo fmt -- --check
     cargo clippy -- -D warnings
   
   # Security audit
   audit:
     cargo audit
     cargo deny check
   
   # Clean build artifacts
   clean:
     cargo clean
     rm -rf dist/
   
   # Generate documentation
   docs:
     cargo doc --no-deps --open
   
   # Watch for changes and rebuild
   watch:
     cargo watch -x build -x test
   
   # Build for all platforms
   build-all:
     cargo build --target x86_64-apple-darwin
     cargo build --target aarch64-apple-darwin
     cargo build --target x86_64-unknown-linux-gnu
     cargo build --target aarch64-unknown-linux-gnu
   
   # Create release packages
   package: release
     #!/usr/bin/env bash
     set -euo pipefail
     
     mkdir -p dist
     
     # macOS
     if [[ "$OSTYPE" == "darwin"* ]]; then
       cp target/release/clipsync dist/clipsync-macos
       tar -czf dist/clipsync-macos-$(uname -m).tar.gz -C dist clipsync-macos
     fi
     
     # Linux
     if [[ "$OSTYPE" == "linux-gnu"* ]]; then
       cp target/release/clipsync dist/clipsync-linux
       tar -czf dist/clipsync-linux-$(uname -m).tar.gz -C dist clipsync-linux
     fi
   
   # Install locally
   install: release
     cargo install --path .
   
   # Setup development environment
   dev-setup:
     rustup component add rustfmt clippy
     cargo install cargo-watch cargo-audit cargo-deny
     pre-commit install
   ```

3. **Create scripts/com.clipsync.plist (macOS launchd)**
   ```xml
   <?xml version="1.0" encoding="UTF-8"?>
   <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
   <plist version="1.0">
   <dict>
       <key>Label</key>
       <string>com.clipsync</string>
       <key>ProgramArguments</key>
       <array>
           <string>/usr/local/bin/clipsync</string>
           <string>--daemon</string>
       </array>
       <key>RunAtLoad</key>
       <true/>
       <key>KeepAlive</key>
       <true/>
       <key>StandardErrorPath</key>
       <string>/tmp/clipsync.err</string>
       <key>StandardOutPath</key>
       <string>/tmp/clipsync.out</string>
   </dict>
   </plist>
   ```

4. **Create scripts/clipsync.service (Linux systemd)**
   ```ini
   [Unit]
   Description=ClipSync - Clipboard Synchronization Service
   After=network.target
   
   [Service]
   Type=simple
   ExecStart=/usr/local/bin/clipsync --daemon
   Restart=always
   RestartSec=10
   User=%i
   Environment="DISPLAY=:0"
   Environment="WAYLAND_DISPLAY=wayland-0"
   
   [Install]
   WantedBy=default.target
   ```

5. **Create .github/workflows/ci.yml**
   ```yaml
   name: CI
   
   on:
     push:
       branches: [ main ]
     pull_request:
       branches: [ main ]
   
   env:
     CARGO_TERM_COLOR: always
   
   jobs:
     test:
       strategy:
         matrix:
           os: [ubuntu-latest, macos-latest]
           rust: [stable, nightly]
       
       runs-on: ${{ matrix.os }}
       
       steps:
       - uses: actions/checkout@v4
       
       - name: Install Rust
         uses: dtolnay/rust-toolchain@master
         with:
           toolchain: ${{ matrix.rust }}
           components: rustfmt, clippy
       
       - name: Cache cargo
         uses: actions/cache@v3
         with:
           path: |
             ~/.cargo/registry
             ~/.cargo/git
             target
           key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
       
       - name: Check formatting
         run: cargo fmt -- --check
       
       - name: Clippy
         run: cargo clippy -- -D warnings
       
       - name: Test
         run: cargo test --all-features
       
       - name: Build
         run: cargo build --release
   ```

## Success Criteria
- Makefile works on both macOS and Linux
- justfile provides convenient development commands
- CI/CD pipeline configured
- Service files for systemd and launchd created
- Package creation automated