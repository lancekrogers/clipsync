#!/usr/bin/env bash
# Stage a public release of the current Cargo version into dist/release/v<version>.
# Builds the macOS app and disk image on this Mac and the Linux archives in Docker
# from a source export of HEAD. Only reads from GitHub; release-publish.sh uploads.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-lib.sh
source "$SCRIPT_DIR/release-lib.sh"

usage() {
  cat <<EOF
Usage: release-stage.sh [--skip-gates]

Stages ClipSync <version> from Cargo.toml into dist/release/v<version>:
  assets/   disk image, Linux archives and SHA256SUMS (uploaded by publish)
  RELEASE_NOTES.md, clipsync.rb (Homebrew cask), release.json
  work/     build intermediates and checks (not uploaded)

Refuses unless the tree is clean, on main and equal to origin/main, both crate
versions agree, and neither the tag nor a GitHub release for it exists.

  --skip-gates   skip fmt/clippy/test (iteration only; publish refuses the result)

Environment:
  CLIPSYNC_RUSTUP_TOOLCHAIN       toolchain for gates and builds (default 1.95.0)
  CLIPSYNC_RELEASE_LINUX_ARCHES   default "x86_64 aarch64"; x86_64 is required
  CLIPSYNC_RELEASE_REPO           owner/repo (default: from the origin URL)
  CLIPSYNC_RELEASE_ROOT           staging root (default: <repo>/dist/release)
  CLIPSYNC_TAP_REPO               Homebrew tap repo (default lancekrogers/homebrew-tap)
  CLIPSYNC_CODESIGN_IDENTITY + notary credentials
                                  sign and notarize through sign-macos.sh; without
                                  them the app is ad-hoc signed and labeled unsigned
EOF
}

SKIP_GATES=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-gates) SKIP_GATES=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; clipsync_release_die "unknown argument: $1" ;;
  esac
done

[[ "$(uname -s)" == "Darwin" ]] || clipsync_release_die "staging builds the macOS disk image, so it runs on macOS"
[[ "$(uname -m)" == "arm64" ]] || clipsync_release_die "staging publishes an Apple Silicon build; run it on an arm64 Mac"

if [[ -n "${CLIPSYNC_PACKAGE_FIXTURE_MODE:-}" || -n "${CLIPSYNC_PACKAGE_SKIP_BUILD:-}" \
   || -n "${CLIPSYNC_PACKAGE_CLI_BIN:-}" || -n "${CLIPSYNC_PACKAGE_GUI_BIN:-}" ]]; then
  clipsync_release_die "fixture or prebuilt binaries cannot be staged; unset CLIPSYNC_PACKAGE_FIXTURE_MODE, CLIPSYNC_PACKAGE_SKIP_BUILD, CLIPSYNC_PACKAGE_CLI_BIN and CLIPSYNC_PACKAGE_GUI_BIN"
fi
for var in CLIPSYNC_PACKAGE_MOCK_BIN CARGO_BUILD_TARGET RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_RUSTFLAGS; do
  [[ -z "${!var:-}" ]] || clipsync_release_die "unset $var: release builds take no tool or compiler overrides from the environment"
done
unset CLIPSYNC_PACKAGE_FORCE CLIPSYNC_PACKAGE_DIST CLIPSYNC_PACKAGE_FIXTURE_PREFIX \
  CLIPSYNC_PACKAGE_CARGO_TARGET CLIPSYNC_PACKAGE_ARCH CARGO_TARGET_DIR

TOOLCHAIN="${CLIPSYNC_RUSTUP_TOOLCHAIN-1.95.0}"
[[ "$TOOLCHAIN" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || clipsync_release_die "release builds need an exact toolchain version such as 1.95.0 (CLIPSYNC_RUSTUP_TOOLCHAIN='$TOOLCHAIN')"
export CLIPSYNC_RUSTUP_TOOLCHAIN="$TOOLCHAIN"

read -r -a LINUX_ARCHES <<<"${CLIPSYNC_RELEASE_LINUX_ARCHES:-x86_64 aarch64}"
HAS_X86=0
for arch in "${LINUX_ARCHES[@]}"; do
  case "$arch" in
    x86_64) HAS_X86=1 ;;
    aarch64) ;;
    *) clipsync_release_die "unsupported Linux arch: $arch (x86_64 or aarch64)" ;;
  esac
done
[[ "$HAS_X86" -eq 1 ]] || clipsync_release_die "the x86_64 Linux archive is required"

clipsync_release_require_tools git gh docker cargo codesign hdiutil python3 shasum

ROOT="$(clipsync_package_project_root)"
clipsync_release_require_clean_tree "$ROOT"
clipsync_release_require_main_at_origin "$ROOT"
VERSION="$(clipsync_release_version "$ROOT")"
TAG="v$VERSION"
SLUG="$(clipsync_release_repo_slug "$ROOT")"
COMMIT="$(git -C "$ROOT" rev-parse HEAD)"
STAGE="$(clipsync_release_stage_dir "$ROOT" "$VERSION")"
[[ ! -e "$STAGE" ]] \
  || clipsync_release_die "staging directory already exists: $STAGE (move or delete it yourself to restage)"
clipsync_release_require_tag_absent "$ROOT" "$TAG"
clipsync_release_require_release_absent "$SLUG" "$TAG"

notary_complete() {
  [[ -n "${CLIPSYNC_NOTARY_PROFILE:-}" ]] \
    || [[ -n "${CLIPSYNC_NOTARY_APPLE_ID:-}" && -n "${CLIPSYNC_NOTARY_TEAM_ID:-}" && -n "${CLIPSYNC_NOTARY_PASSWORD:-}" ]]
}
notary_any() {
  [[ -n "${CLIPSYNC_NOTARY_PROFILE:-}${CLIPSYNC_NOTARY_APPLE_ID:-}${CLIPSYNC_NOTARY_TEAM_ID:-}${CLIPSYNC_NOTARY_PASSWORD:-}" ]]
}
if [[ -n "${CLIPSYNC_CODESIGN_IDENTITY:-}" ]]; then
  [[ "$CLIPSYNC_CODESIGN_IDENTITY" != "-" ]] \
    || clipsync_release_die "CLIPSYNC_CODESIGN_IDENTITY=- is ad-hoc signing; unset it to stage an unsigned build"
  notary_complete || clipsync_release_die "CLIPSYNC_CODESIGN_IDENTITY is set but notarization credentials are incomplete; set CLIPSYNC_NOTARY_PROFILE (or Apple ID, team ID and password), or unset the identity to stage an unsigned build"
  SIGNING="developer-id"
elif notary_any; then
  clipsync_release_die "notarization credentials are set without CLIPSYNC_CODESIGN_IDENTITY; set both or neither"
else
  SIGNING="unsigned"
fi

clipsync_release_log "staging ClipSync $VERSION ($TAG) from $COMMIT for $SLUG"
clipsync_release_log "macOS signing: $SIGNING; Linux: ${LINUX_ARCHES[*]}"
if [[ "$SIGNING" == "unsigned" ]]; then
  clipsync_release_log "No Developer ID credentials: the app is ad-hoc signed and labeled unsigned."
fi

GATES="passed"
if [[ "$SKIP_GATES" -eq 1 ]]; then
  GATES="skipped"
  cat >&2 <<EOF
[release] ################################################################
[release] WARNING: --skip-gates: fmt, clippy and tests were NOT run.
[release] release.json records quality_gates=skipped and publish will refuse it.
[release] ################################################################
EOF
else
  run_gate() {
    local name="$1"
    shift
    clipsync_release_log "gate: cargo +$TOOLCHAIN $*"
    (cd "$ROOT" && command cargo "+$TOOLCHAIN" "$@") || clipsync_release_die "quality gate failed: $name"
  }
  run_gate fmt fmt --all -- --check
  run_gate clippy clippy --workspace --all-features -- -D warnings
  run_gate test test --workspace --all-features
fi

# Gates must not have changed the tree or HEAD.
clipsync_release_require_clean_tree "$ROOT"
[[ "$(git -C "$ROOT" rev-parse HEAD)" == "$COMMIT" ]] || clipsync_release_die "HEAD moved during staging"

mkdir -p "$STAGE/assets" "$STAGE/work"
WORK="$STAGE/work"
ASSET_ARGS=()

git -C "$ROOT" archive --format=tar -o "$WORK/source.tar" "$COMMIT"

# --- macOS (from the same export as Linux, so local-only files cannot leak in) ---
mkdir -p "$WORK/src"
tar -xf "$WORK/source.tar" -C "$WORK/src"
CLIPSYNC_PACKAGE_RELEASE=1 \
CLIPSYNC_PACKAGE_RELEASE_SIGNING="$SIGNING" \
CLIPSYNC_PACKAGE_DIST="$WORK" \
CLIPSYNC_PACKAGE_GIT_COMMIT="$COMMIT" \
CARGO_TARGET_DIR="$WORK/target" \
CARGO_BUILD_BUILD_DIR="$WORK/target" \
  bash "$WORK/src/scripts/package/build-macos-companion.sh"

MAC_DIR="$WORK/macos"
MAC_APP="$MAC_DIR/ClipSync.app"
MAC_MANIFEST="$MAC_DIR/MANIFEST-${VERSION}-arm64.json"
DMG_NAME="$(clipsync_release_json_get "$MAC_MANIFEST" dmg)"
[[ -n "$DMG_NAME" && -f "$MAC_DIR/$DMG_NAME" ]] || clipsync_release_die "macOS build did not produce its disk image"

if [[ "$SIGNING" == "developer-id" ]]; then
  bash "$SCRIPT_DIR/sign-macos.sh" --app "$MAC_APP" --dmg "$MAC_DIR/$DMG_NAME"
  bash "$SCRIPT_DIR/verify.sh" --acceptance "$MAC_DIR/ACCEPTANCE-ClipSync.json"
  grep -q '"status": "accepted"' "$MAC_DIR/ACCEPTANCE-ClipSync.json" \
    || clipsync_release_die "notarization was not accepted"
  # Confirm with the system tools rather than trusting ACCEPTANCE alone.
  SIGN_INFO="$(/usr/bin/codesign -dv --verbose=2 "$MAC_APP" 2>&1)" || SIGN_INFO=""
  [[ "$SIGN_INFO" == *$'\n'"Authority=Developer ID Application:"* ]] \
    || clipsync_release_die "the signed app is not signed by a Developer ID Application identity"
  /usr/bin/xcrun stapler validate "$MAC_APP" >/dev/null \
    || clipsync_release_die "the signed app has no valid stapled notarization ticket"
  GK="$(/usr/sbin/spctl --assess --type execute -vv "$MAC_APP" 2>&1)" \
    || clipsync_release_die "Gatekeeper rejects the signed app"
  [[ "$GK" == *"source=Notarized Developer ID"* ]] \
    || clipsync_release_die "Gatekeeper does not report the app as notarized"
  python3 -I - "$MAC_MANIFEST" <<'PY'
import json, sys
path = sys.argv[1]
data = json.load(open(path))
data["notarized"] = True
data["public_release_ready"] = True
open(path, "w").write(json.dumps(data, indent=2) + "\n")
PY
fi

bash "$SCRIPT_DIR/verify.sh" --macos "$MAC_APP" --manifest "$MAC_MANIFEST"

# Check the app as it ships: from inside the disk image.
MOUNT="$(mktemp -d "${TMPDIR:-/tmp}/clipsync-release-dmg.XXXXXX")"
hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$MOUNT" "$MAC_DIR/$DMG_NAME" >/dev/null \
  || clipsync_release_die "could not attach $DMG_NAME"
DMG_OK=1
codesign --verify --deep --strict "$MOUNT/ClipSync.app" || DMG_OK=0
bash "$SCRIPT_DIR/verify.sh" --macos "$MOUNT/ClipSync.app" || DMG_OK=0
hdiutil detach "$MOUNT" >/dev/null || hdiutil detach -force "$MOUNT" >/dev/null || true
rmdir "$MOUNT" 2>/dev/null || true
[[ "$DMG_OK" -eq 1 ]] || clipsync_release_die "the app inside $DMG_NAME does not verify"

cp "$MAC_DIR/$DMG_NAME" "$STAGE/assets/$DMG_NAME"
ASSET_ARGS+=(--asset "$DMG_NAME=macos:arm64")

# --- Linux (Docker, from the source export) ---
CONTAINER=""
remove_container() {
  if [[ -n "$CONTAINER" ]]; then
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    CONTAINER=""
  fi
}
trap remove_container EXIT

IMAGE="clipsync-release-linux:ubuntu22.04-rust${TOOLCHAIN}"
docker build --platform linux/arm64 --build-arg "RUST_TOOLCHAIN=$TOOLCHAIN" \
  -t "$IMAGE" - <"$SCRIPT_DIR/Dockerfile.release-linux" \
  || clipsync_release_die "docker build of the Linux release image failed"

GLIBC_MAX=""
NEEDED_ALL=()
LINUX_ROWS=""
for arch in "${LINUX_ARCHES[@]}"; do
  case "$arch" in
    x86_64) elf_machine="x86-64" ;;
    aarch64) elf_machine="ARM aarch64" ;;
  esac
  clipsync_release_log "building Linux $arch in $IMAGE"
  CONTAINER="$(docker create --platform linux/arm64 \
    -e "CLIPSYNC_RELEASE_LINUX_ARCH=$arch" \
    -e "CLIPSYNC_PACKAGE_GIT_COMMIT=$COMMIT" \
    -e "CLIPSYNC_RUSTUP_TOOLCHAIN=$TOOLCHAIN" \
    --mount "type=volume,source=clipsync-release-cargo-registry,target=/usr/local/cargo/registry" \
    "$IMAGE" bash -c 'set -euo pipefail; mkdir -p /work/src; tar -xmf /work/source.tar -C /work/src; exec bash /work/src/scripts/package/release-linux-container.sh')"
  docker cp "$WORK/source.tar" "$CONTAINER:/work/source.tar" >/dev/null
  docker start -a "$CONTAINER" || clipsync_release_die "Linux $arch build failed"
  mkdir -p "$WORK/linux-$arch"
  docker cp "$CONTAINER:/out/." "$WORK/linux-$arch/" >/dev/null
  remove_container

  manifest="$WORK/linux-$arch/linux/MANIFEST-${VERSION}-${arch}.json"
  [[ -f "$manifest" ]] || clipsync_release_die "Linux $arch build wrote no manifest"
  archive="$(clipsync_release_json_get "$manifest" archive)"
  bundle="$(clipsync_release_json_get "$manifest" bundle)"
  [[ -f "$WORK/linux-$arch/linux/$archive" ]] || clipsync_release_die "Linux $arch archive missing"

  # Verify what ships: the extracted archive, not the build directory.
  mkdir -p "$WORK/check-linux-$arch"
  tar -xzf "$WORK/linux-$arch/linux/$archive" -C "$WORK/check-linux-$arch"
  bash "$SCRIPT_DIR/verify.sh" --linux "$WORK/check-linux-$arch/$bundle" --manifest "$manifest"
  for bin in clipsync clipsync-desktop; do
    [[ "$(file -b "$WORK/check-linux-$arch/$bundle/bin/$bin")" == *"$elf_machine"* ]] \
      || clipsync_release_die "Linux $arch $bin is not an $elf_machine ELF binary"
  done

  glibc="$(clipsync_release_json_get "$manifest" glibc_required)"
  [[ -n "$glibc" ]] || clipsync_release_die "Linux $arch manifest has no glibc_required"
  GLIBC_MAX="$(printf '%s\n%s\n' "$GLIBC_MAX" "$glibc" | sed '/^$/d' | sort -V | tail -1)"
  while IFS= read -r lib; do
    [[ -n "$lib" ]] && NEEDED_ALL+=("$lib")
  done < <(python3 -I -c 'import json,sys; print("\n".join(json.load(open(sys.argv[1]))["needed_libraries"]))' "$manifest")

  cp "$WORK/linux-$arch/linux/$archive" "$STAGE/assets/$archive"
  ASSET_ARGS+=(--asset "$archive=linux:$arch")
  LINUX_ROWS+="| \`$archive\` | Linux $arch, glibc $glibc or newer |"$'\n'
done
trap - EXIT

# --- Checksums ---
ASSET_NAMES=()
for ((i = 1; i < ${#ASSET_ARGS[@]}; i += 2)); do
  ASSET_NAMES+=("${ASSET_ARGS[i]%%=*}")
done
(cd "$STAGE/assets" && shasum -a 256 "${ASSET_NAMES[@]}" >SHA256SUMS)
(cd "$STAGE/assets" && shasum -a 256 -c SHA256SUMS >/dev/null) || clipsync_release_die "SHA256SUMS does not verify"
ASSET_ARGS+=(--asset "SHA256SUMS=any:any")

# --- Homebrew cask ---
DMG_SHA="$(clipsync_release_sha256 "$STAGE/assets/$DMG_NAME")"
TAP_REPO="${CLIPSYNC_TAP_REPO:-lancekrogers/homebrew-tap}"
TAP_NAME="${TAP_REPO%%/*}/${TAP_REPO#*/homebrew-}"
SIGNING_BLOCK="$([[ "$SIGNING" == "unsigned" ]] && echo UNSIGNED || echo SIGNED)"
DMG_URL_NAME="$(python3 -I -c 'import sys; print(sys.argv[1].replace(sys.argv[2], "#{version}"))' "$DMG_NAME" "$VERSION")"
python3 -I "$SCRIPT_DIR/release_render.py" template "$SCRIPT_DIR/clipsync.rb.in" "$STAGE/clipsync.rb" \
  --set "VERSION=$VERSION" --set "DMG_SHA256=$DMG_SHA" --set "REPO=$SLUG" \
  --set "DMG_URL_NAME=$DMG_URL_NAME" \
  --enable "$SIGNING_BLOCK"
if command -v ruby >/dev/null 2>&1; then
  ruby -c "$STAGE/clipsync.rb" >/dev/null || clipsync_release_die "generated cask is not valid Ruby"
fi

# --- Release notes ---
if [[ "$SIGNING" == "unsigned" ]]; then
  MAC_ROW="| \`$DMG_NAME\` | macOS on Apple Silicon (not notarized; see below) |"
else
  MAC_ROW="| \`$DMG_NAME\` | macOS on Apple Silicon |"
fi
# Libraries beyond the glibc/libgcc runtime every Linux system has.
NEEDED_TEXT="$(printf '%s\n' ${NEEDED_ALL[@]+"${NEEDED_ALL[@]}"} \
  | { grep -Ev '^(ld-linux.*|libc\.so\..*|libm\.so\..*|libgcc_s\.so\..*|libpthread\.so\..*|libdl\.so\..*|librt\.so\..*|libutil\.so\..*)$' || true; } \
  | sed '/^$/d' | sort -u | sed 's/.*/`&`/' | paste -sd, - | sed 's/,/, /g')"
[[ -n "$NEEDED_TEXT" ]] || NEEDED_TEXT="nothing else"
NOTE_BLOCKS=(--enable "$SIGNING_BLOCK")
for arch in "${LINUX_ARCHES[@]}"; do
  [[ "$arch" == "aarch64" ]] && NOTE_BLOCKS+=(--enable AARCH64)
done
python3 -I "$SCRIPT_DIR/release_render.py" template "$SCRIPT_DIR/release-notes.md.in" "$STAGE/RELEASE_NOTES.md" \
  --set "VERSION=$VERSION" --set "TAG=$TAG" --set "REPO=$SLUG" --set "COMMIT=$COMMIT" \
  --set "TOOLCHAIN=$TOOLCHAIN" --set "TAP=$TAP_NAME" --set "DMG=$DMG_NAME" \
  --set "GLIBC=$GLIBC_MAX" --set "NEEDED=$NEEDED_TEXT" \
  --set "ASSET_TABLE=${MAC_ROW}"$'\n'"${LINUX_ROWS%$'\n'}"$'\n'"| \`SHA256SUMS\` | Checksums for the files above |" \
  "${NOTE_BLOCKS[@]}"

# --- release.json: written last; publish treats its absence as an incomplete stage ---
ARCH_JSON="$(printf '%s\n' "${LINUX_ARCHES[@]}" | python3 -I -c 'import json,sys; print(json.dumps(sys.stdin.read().split()))')"
NOTARIZED=false
[[ "$SIGNING" == "developer-id" ]] && NOTARIZED=true
python3 -I "$SCRIPT_DIR/release_render.py" manifest "$STAGE/release.json" \
  --field "version=$VERSION" --field "tag=$TAG" --field "repo=$SLUG" --field "commit=$COMMIT" \
  --field "signing=$SIGNING" --field "quality_gates=$GATES" --field "rust_toolchain=$TOOLCHAIN" \
  --field "notes=RELEASE_NOTES.md" --field "cask=clipsync.rb" \
  --field "cask_sha256=$(clipsync_release_sha256 "$STAGE/clipsync.rb")" \
  --json-field "release_build=true" --json-field "fixture_mode=false" \
  --json-field "notarized=$NOTARIZED" \
  --json-field "linux={\"build_image\": \"ubuntu:22.04\", \"glibc_required\": \"$GLIBC_MAX\", \"arches\": $ARCH_JSON}" \
  "${ASSET_ARGS[@]}"

clipsync_release_log "staged $TAG in $STAGE"
(cd "$STAGE/assets" && sed 's/^/[release]   /' SHA256SUMS)
clipsync_release_log "macOS signing: $SIGNING; quality gates: $GATES; Linux glibc >= $GLIBC_MAX"
clipsync_release_log "Docker keeps $IMAGE and volume clipsync-release-cargo-registry as build caches;"
clipsync_release_log "  free them with: docker rmi $IMAGE && docker volume rm clipsync-release-cargo-registry"
clipsync_release_log "review $STAGE/RELEASE_NOTES.md, then: just companion-release-publish"
