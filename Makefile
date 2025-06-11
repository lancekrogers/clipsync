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

# Build for all platforms
build-all:
	cargo build --target x86_64-apple-darwin
	cargo build --target aarch64-apple-darwin
	cargo build --target x86_64-unknown-linux-gnu
	cargo build --target aarch64-unknown-linux-gnu