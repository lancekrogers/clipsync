# Build and check both interfaces; daemon-only builds still use the root default member.
companion-verify:
  cargo +1.95.0 fmt --all -- --check
  cargo +1.95.0 clippy --workspace --all-features -- -D warnings
  cargo +1.95.0 test --workspace --all-features
  cargo +1.95.0 test --doc

# Native windows + PTY, using only a named test pasteboard and temporary stores.
companion-test-macos:
  cargo +1.95.0 build --locked --workspace --all-features --bins --example companion_fixture
  uv run --with msgpack --with pyte python scripts/test/companion-macos.py

# Real X11 and headless Sway GUI/TUI in a disposable container, no host clipboard.
companion-test-linux:
  mkdir -p target/companion-linux-evidence
  docker build -f scripts/test/Dockerfile.companion -t clipsync-companion-test:local .
  docker run --rm --mount type=bind,source="{{justfile_directory()}}",target=/source,readonly --mount type=volume,source=clipsync-repair-cargo,target=/usr/local/cargo/registry --mount type=volume,source=clipsync-companion-target,target=/work/target --mount type=bind,source="{{justfile_directory()}}/target/companion-linux-evidence",target=/evidence clipsync-companion-test:local bash /source/scripts/test/companion-container.sh

# Duplicate launch / crash recovery against a separate private macOS fixture.
companion-test-instance-macos:
  cargo +1.95.0 build --locked --workspace --all-features --bins --example companion_fixture
  uv run --with msgpack python scripts/test/companion_instance_ui.py target/companion-evidence/instance

# Real process crash/restart; private Mac pasteboard or disposable Linux desktop.
companion-test-restart:
  cargo +1.95.0 build --locked --features tui,integration-tests --bin clipsync --example companion_fixture
  uv run --with pyte python scripts/test/companion_restart.py '{{env_var_or_default("CARGO_TARGET_DIR", "target")}}/debug/examples/companion_fixture' '{{env_var_or_default("CARGO_TARGET_DIR", "target")}}/debug/clipsync' target/companion-evidence/restart
