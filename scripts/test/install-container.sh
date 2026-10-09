#!/usr/bin/env bash
set -euo pipefail
tar -C /source --exclude=target --exclude=.git --exclude=.agents --exclude=.claude --exclude=.grok -cf - . | tar -C /work -xf -
cd /work
cargo build --locked --bin clipsync
python3 scripts/test/install_end_to_end.py target/debug/clipsync
