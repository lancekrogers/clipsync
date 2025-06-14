# ClipSync Makefile
# Run 'make help' for a list of available commands

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

.PHONY: all build release test clean install uninstall fmt lint bench check pre-commit help

# Default target - show help if no target specified
all: help

# Display help information
help:
	@echo "ClipSync Makefile Commands:"
	@echo ""
	@echo "  make help           Show this help message"
	@echo "  make build          Build debug binary for current platform"
	@echo "  make release        Build optimized release binary"
	@echo "  make check          Run format and lint checks (fast)"
	@echo "  make pre-commit     Run all pre-commit checks"
	@echo "  make test           Run all tests"
	@echo "  make test-integration Run integration tests"
	@echo "  make fmt            Check code formatting"
	@echo "  make fmt-fix        Fix code formatting issues"
	@echo "  make lint           Run clippy linter and audit"
	@echo "  make bench          Run benchmarks"
	@echo "  make clean          Remove build artifacts"
	@echo "  make install        Install clipsync system-wide"
	@echo "  make uninstall      Remove clipsync from system"
	@echo "  make package        Create distribution package"
	@echo "  make build-all      Build for all supported platforms"
	@echo ""
	@echo "Platform: $(PLATFORM) ($(TARGET))"
	@echo ""

# Run all pre-commit checks
pre-commit:
	@./scripts/pre-commit-checks.sh

# Quick check - formatting and linting only
check:
	@if [ -f .env ]; then source .env; fi && \
	unset OPENSSL_LIB_DIR OPENSSL_INCLUDE_DIR && \
	export OPENSSL_ROOT_DIR=/opt/homebrew/opt/openssl@3 && \
	export OPENSSL_LIB_DIR=/opt/homebrew/opt/openssl@3/lib && \
	export OPENSSL_INCLUDE_DIR=/opt/homebrew/opt/openssl@3/include && \
	$(CARGO) fmt --check && \
	$(CARGO) clippy -- -D warnings && \
	$(CARGO) check --all-targets

build:
	$(CARGO) build --target $(TARGET)

release:
	$(CARGO) build --release --target $(TARGET)

test:
	$(CARGO) test
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

# Build for all platforms
build-all:
	cargo build --target x86_64-apple-darwin
	cargo build --target aarch64-apple-darwin
	cargo build --target x86_64-unknown-linux-gnu
	cargo build --target aarch64-unknown-linux-gnu