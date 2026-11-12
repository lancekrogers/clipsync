#!/usr/bin/env bash
# Runs inside the Linux release build container started by release-stage.sh,
# from a source export of the staged commit. Writes the archive to /out/linux.
set -euo pipefail

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [[ ! -f /.dockerenv && ! -f /run/.containerenv ]]; then
  echo "release-linux-container.sh only runs inside the release build container" >&2
  exit 1
fi
: "${CLIPSYNC_PACKAGE_GIT_COMMIT:?the staged commit must be passed in}"
ARCH="${CLIPSYNC_RELEASE_LINUX_ARCH:?x86_64 or aarch64}"

unset CLIPSYNC_PACKAGE_FIXTURE_MODE CLIPSYNC_PACKAGE_SKIP_BUILD \
  CLIPSYNC_PACKAGE_CLI_BIN CLIPSYNC_PACKAGE_GUI_BIN CLIPSYNC_PACKAGE_FORCE
export CLIPSYNC_PACKAGE_RELEASE=1
export CLIPSYNC_PACKAGE_DIST=/out
export CLIPSYNC_PACKAGE_ARCH="$ARCH"
export CARGO_TARGET_DIR=/work/target

case "$ARCH:$(uname -m)" in
  aarch64:aarch64) ;;
  x86_64:aarch64)
    export CLIPSYNC_PACKAGE_CARGO_TARGET=x86_64-unknown-linux-gnu
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc
    export CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc
    export CXX_x86_64_unknown_linux_gnu=x86_64-linux-gnu-g++
    export AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar
    export PKG_CONFIG_ALLOW_CROSS=1
    export PKG_CONFIG_LIBDIR=/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig
    export X86_64_UNKNOWN_LINUX_GNU_OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu
    export X86_64_UNKNOWN_LINUX_GNU_OPENSSL_INCLUDE_DIR=/usr/include
    ;;
  *) echo "unsupported build: $ARCH on $(uname -m)" >&2; exit 1 ;;
esac

mkdir -p /out
exec bash "$SRC/scripts/package/build-linux-companion.sh"
