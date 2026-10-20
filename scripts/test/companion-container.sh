#!/usr/bin/env bash
set -euo pipefail
# Override the repository stable channel inside this pinned test image.
export RUSTUP_TOOLCHAIN=1.95.0
# Source is read-only; every configuration, key and clipboard stays in this container.
tar -C /source --exclude=target --exclude=target-package --exclude=.git --exclude=.agents --exclude=.claude --exclude=dist -cf - . | tar -C /work -xf -
cd /work
cargo test --locked --all-features -- --test-threads=1
CLIPSYNC_DISPOSABLE_DESKTOP=1 cargo test --locked --all-features --test companion_discovery -- --ignored --test-threads=1
cargo test --locked --doc
cargo build --locked --workspace --all-features --bins --example companion_fixture
mkdir -p /evidence
chmod 777 /evidence
ldd target/debug/clipsync-desktop > /evidence/runtime-libraries.txt
! grep -q "not found" /evidence/runtime-libraries.txt
bash scripts/package_companion.sh test > /evidence/package-fixtures.log
bash scripts/test/companion-linux.sh x11 /evidence/x11
runuser -u nobody -- dbus-run-session -- bash scripts/test/companion-linux.sh wayland /evidence/wayland
