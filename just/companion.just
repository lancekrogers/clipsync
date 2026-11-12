# Companion packaging recipes (local-dev / unsigned by default).
# Root justfile already imports this file.

# Optional toolchain pin for GUI (Rust 1.95). Does not change rustup default.
companion_toolchain := env_var_or_default("CLIPSYNC_RUSTUP_TOOLCHAIN", "1.95.0")

# Build matching CLI (tui) + GUI without integration-tests / ui-test (--locked).
companion-package-binaries:
  #!/usr/bin/env bash
  set -euo pipefail
  export CLIPSYNC_RUSTUP_TOOLCHAIN="{{companion_toolchain}}"
  ./scripts/package_companion.sh binaries

# Current-host companion package (macOS .app+DMG or Linux user archive).
# Artifacts are UNSIGNED-DEVELOPMENT unless companion-sign-macos succeeds.
companion-package:
  #!/usr/bin/env bash
  set -euo pipefail
  export CLIPSYNC_RUSTUP_TOOLCHAIN="{{companion_toolchain}}"
  ./scripts/package_companion.sh all

# macOS ClipSync.app + UNSIGNED-DEVELOPMENT DMG only.
companion-package-macos:
  #!/usr/bin/env bash
  set -euo pipefail
  export CLIPSYNC_RUSTUP_TOOLCHAIN="{{companion_toolchain}}"
  ./scripts/package_companion.sh macos

# Linux user-local archive (requires Linux host, or FIXTURE_MODE for script tests).
companion-package-linux:
  #!/usr/bin/env bash
  set -euo pipefail
  export CLIPSYNC_RUSTUP_TOOLCHAIN="{{companion_toolchain}}"
  ./scripts/package_companion.sh linux

# Static package verification (pass paths via args).
companion-package-verify *ARGS:
  ./scripts/package_companion.sh verify {{ARGS}}

# Fixture runs are NOT real ELF/Mach-O release proof. No host service changes, no network writes.
# Isolated packaging and release-command tests (fixture prefix / tempdirs, stub gh/cargo/docker).
companion-package-test:
  ./scripts/package_companion.sh test

# Developer ID sign + notarize — hard-gated on CLIPSYNC_CODESIGN_IDENTITY and
# notary credentials. Recreates DMG from stapled .app. Refuses without credentials.
companion-sign-macos *ARGS:
  ./scripts/package_companion.sh sign-macos {{ARGS}}

# Runs gates, builds the macOS DMG (ad-hoc signed unless Developer ID credentials are set) and
# Linux archives in Docker, writes SHA256SUMS, release notes and the Homebrew cask. Read-only
# against GitHub. --skip-gates is for iteration only; publish refuses a stage made with it.
# Stage a public release of the current Cargo version into dist/release/v<version>.
companion-release *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  export CLIPSYNC_RUSTUP_TOOLCHAIN="{{companion_toolchain}}"
  ./scripts/package_companion.sh release {{ARGS}}

# Publishing the draft (which creates the tag) stays a manual step.
# Upload the staged release as a DRAFT GitHub release.
companion-release-publish:
  ./scripts/package_companion.sh release-publish

# Refuses while the release is a draft or the public DMG checksum differs from the staged one.
# After the release is published, push the staged cask to the Homebrew tap.
companion-release-tap:
  ./scripts/package_companion.sh release-tap
