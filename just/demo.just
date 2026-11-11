# Stills for review land in target/demo/.
# Record docs/assets/companion-tui.gif with VHS against the isolated fixture (macOS).
demo-tui: (_record-tui "companion-tui")

# Record docs/assets/companion-tui-history-remove.gif: delete one copy, then clear all history.
demo-tui-history-remove: (_record-tui "companion-tui-history-remove")

[private]
_record-tui name:
  #!/usr/bin/env bash
  set -euo pipefail
  cargo +1.95.0 build --locked --features tui,integration-tests --bin clipsync --example companion_fixture
  mkdir -p target/demo
  if ! vhs demos/{{name}}.tape; then
    kill -INT "$(cat target/demo/fixture.pid)" 2>/dev/null || true
    exit 1
  fi
  ls -lh docs/assets/{{name}}.gif
