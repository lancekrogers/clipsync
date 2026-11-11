# Record docs/assets/companion-tui.gif with VHS against the isolated fixture (macOS).
# Stills for review land in target/demo/.
demo-tui:
  #!/usr/bin/env bash
  set -euo pipefail
  cargo +1.95.0 build --locked --features tui,integration-tests --bin clipsync --example companion_fixture
  mkdir -p target/demo
  if ! vhs demos/companion-tui.tape; then
    kill -INT "$(cat target/demo/fixture.pid)" 2>/dev/null || true
    exit 1
  fi
  ls -lh docs/assets/companion-tui.gif
