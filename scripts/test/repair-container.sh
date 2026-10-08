#!/usr/bin/env bash
set -euo pipefail
# Run inside the disposable repair-test container, with the repository at /source.
tar -C /source --exclude=target --exclude=.git --exclude=.agents --exclude=.claude --exclude=.grok -cf - . | tar -C /work -xf -
cd /work
cargo test --locked --all-features -- --test-threads=1
cargo test --locked --doc
cargo build --locked --bin clipsync
python3 scripts/test/desktop_sync.py

runuser -u nobody -- python3 scripts/test/desktop_sync.py --wayland
