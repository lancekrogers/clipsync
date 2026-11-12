#!/usr/bin/env bash
# Build the clipsync-bin AUR package from the staged release, test it in an Arch
# Linux container, and push it to the AUR once the GitHub release is public.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-lib.sh
source "$SCRIPT_DIR/release-lib.sh"

MODE=push
case "${1:-}" in
  "") ;;
  --check) MODE=check ;;
  -h|--help)
    cat <<EOF
Usage: release-aur.sh [--check]

Renders the clipsync-bin PKGBUILD from dist/release/v<version>/release.json,
builds and installs it in an Arch Linux container, and writes PKGBUILD and
.SRCINFO to dist/release/v<version>/aur/.

  --check   Test with the staged archives. Works before the release is public;
            contacts neither GitHub nor the AUR.
  (none)    Refuses unless the GitHub release is published and its archives
            match the staged checksums. The container downloads the public
            files, then PKGBUILD and .SRCINFO are pushed to the AUR.

Environment:
  CLIPSYNC_AUR_URL         default ssh://aur@aur.archlinux.org/clipsync-bin.git
  CLIPSYNC_AUR_SSH_KEY     private key for the AUR account (default: ssh config)
  CLIPSYNC_AUR_MAINTAINER  "Name <email>" for the PKGBUILD and the AUR commit
                           (default: Obedience Corp <contact@obediencecorp.com>)
  CLIPSYNC_AUR_IMAGE       default archlinux:base-devel
EOF
    exit 0
    ;;
  *) clipsync_release_die "unknown argument: $1" ;;
esac
[[ $# -le 1 ]] || clipsync_release_die "unexpected arguments: $*"

clipsync_release_require_tools git gh curl docker python3

ROOT="$(clipsync_package_project_root)"
VERSION="$(clipsync_release_version "$ROOT")"
TAG="v$VERSION"
SLUG="$(clipsync_release_repo_slug "$ROOT")"
STAGE="$(clipsync_release_stage_dir "$ROOT" "$VERSION")"
MANIFEST="$STAGE/release.json"
AUR_URL="${CLIPSYNC_AUR_URL:-ssh://aur@aur.archlinux.org/clipsync-bin.git}"
MAINTAINER="${CLIPSYNC_AUR_MAINTAINER:-Obedience Corp <contact@obediencecorp.com>}"
IMAGE="${CLIPSYNC_AUR_IMAGE:-archlinux:base-devel}"

[[ "$MAINTAINER" =~ ^([^\<\>]*[^\<\>\ ])\ \<([^\<\>\ @]+@[^\<\>\ ]+)\>$ ]] \
  || clipsync_release_die "CLIPSYNC_AUR_MAINTAINER must look like 'Name <email>' (got '$MAINTAINER')"
AUR_NAME="${BASH_REMATCH[1]}"
AUR_EMAIL="${BASH_REMATCH[2]}"

[[ -f "$MANIFEST" ]] || clipsync_release_die "incomplete staging directory (no release.json): $STAGE"
[[ "$(clipsync_release_json_get "$MANIFEST" version)" == "$VERSION" ]] \
  || clipsync_release_die "release.json in $STAGE is not for version $VERSION"
COMMIT="$(clipsync_release_json_get "$MANIFEST" commit)"
[[ "$COMMIT" =~ ^[0-9a-f]{40}$ ]] || clipsync_release_die "release.json has no commit"
if [[ "$MODE" == push && "$(clipsync_release_json_get "$MANIFEST" quality_gates)" != "passed" ]]; then
  clipsync_release_die "release.json does not record passed quality gates"
fi

# name and sha256 of each staged Linux archive, x86_64 first.
ARCHIVES="$(python3 -I -c '
import json, sys
assets = {a["arch"]: a for a in json.load(open(sys.argv[1]))["assets"] if a["os"] == "linux"}
for arch in ("x86_64", "aarch64"):
    if arch in assets:
        print(arch, assets[arch]["name"], assets[arch]["sha256"])' "$MANIFEST")"
[[ -n "$ARCHIVES" ]] || clipsync_release_die "release.json lists no Linux archives"
X86_SHA="" ARM_SHA=""
ARCHIVE_NAMES=()
while read -r arch name sha; do
  [[ "$name" == "clipsync-$VERSION-linux-$arch.tar.gz" ]] \
    || clipsync_release_die "unexpected Linux archive name in release.json: $name"
  [[ -f "$STAGE/assets/$name" ]] || clipsync_release_die "staged archive missing: $STAGE/assets/$name"
  [[ "$(clipsync_release_sha256 "$STAGE/assets/$name")" == "$sha" ]] \
    || clipsync_release_die "staged $name changed after staging (sha256 differs from release.json)"
  case "$arch" in
    x86_64) X86_SHA="$sha" ;;
    aarch64) ARM_SHA="$sha" ;;
  esac
  ARCHIVE_NAMES+=("$name")
done <<<"$ARCHIVES"
[[ -n "$X86_SHA" ]] || clipsync_release_die "release.json lists no x86_64 Linux archive"

if [[ "$MODE" == push ]]; then
  DRAFT="$(gh release view "$TAG" --repo "$SLUG" --json isDraft --jq '.isDraft')" \
    || clipsync_release_die "no GitHub release $TAG in $SLUG (or GitHub is unreachable); publish the draft first"
  [[ "$DRAFT" == "false" ]] \
    || clipsync_release_die "release $TAG is still a draft; publish it before updating the AUR"
  TAG_REFS="$(git -C "$ROOT" ls-remote origin "refs/tags/$TAG" "refs/tags/$TAG^{}")" \
    || clipsync_release_die "could not list tags on origin"
  TAG_SHA="$(awk 'END {print $1}' <<<"$TAG_REFS")"
  [[ "$TAG_SHA" == "$COMMIT" ]] \
    || clipsync_release_die "tag $TAG on origin points at '${TAG_SHA:-nothing}', not the staged commit $COMMIT"
fi

mkdir -p "$STAGE/work"
WORK="$(mktemp -d "$STAGE/work/aur.XXXXXX")"
# ctx/ becomes /work in the container: the check script plus the package directory.
PKGDIR="$WORK/ctx/pkg"
mkdir -p "$PKGDIR" "$WORK/sources"
cp "$SCRIPT_DIR/aur-container-check.sh" "$WORK/ctx/"

# License texts come from the tagged commit; the Linux archive does not carry them.
LICENSE_ARGS=()
for license in LICENSE-MIT LICENSE-APACHE; do
  git -C "$ROOT" show "$COMMIT:$license" >"$WORK/sources/clipsync-bin-$VERSION-$license" 2>/dev/null \
    || clipsync_release_die "cannot read $license at the staged commit $COMMIT (git fetch origin, then retry)"
  key="$(tr '-' '_' <<<"$license")_SHA256"
  LICENSE_ARGS+=(--set "$key=$(clipsync_release_sha256 "$WORK/sources/clipsync-bin-$VERSION-$license")")
done
for name in "${ARCHIVE_NAMES[@]}"; do
  cp "$STAGE/assets/$name" "$WORK/sources/$name"
done

if [[ "$MODE" == push ]]; then
  # Same files at the URLs the PKGBUILD uses, or nothing is pushed.
  mkdir -p "$WORK/public"
  for name in "${ARCHIVE_NAMES[@]}"; do
    url="https://github.com/$SLUG/releases/download/$TAG/$name"
    clipsync_release_log "downloading $url"
    curl -fsSL --retry 3 -o "$WORK/public/$name" "$url" \
      || clipsync_release_die "could not download the public archive from $url"
  done
  for license in LICENSE-MIT LICENSE-APACHE; do
    url="https://raw.githubusercontent.com/$SLUG/$TAG/$license"
    curl -fsSL --retry 3 -o "$WORK/public/clipsync-bin-$VERSION-$license" "$url" \
      || clipsync_release_die "could not download $url"
  done
  for file in "$WORK/public"/*; do
    [[ "$(clipsync_release_sha256 "$file")" == "$(clipsync_release_sha256 "$WORK/sources/${file##*/}")" ]] \
      || clipsync_release_die "public ${file##*/} does not match the staged release"
  done
fi

# libcrypto.so.3 is linked. Depend on the soname: Arch ships it as openssl-3 (openssl is
# 4.x there) while Arch Linux ARM still ships it as openssl. The X11, Wayland, xkbcommon
# and GL libraries are loaded by the GUI at runtime, which the container check verifies;
# namcap cannot see them and calls them possibly unneeded.
DEPENDS="'glibc' 'libgcc' 'libcrypto.so=3-64' 'libx11' 'libxcursor' 'libxi' 'libxrender'
         'libxkbcommon' 'libxkbcommon-x11' 'wayland' 'libglvnd' 'hicolor-icon-theme'"
RENDER_ARGS=(--set "VERSION=$VERSION" --set "REPO=$SLUG" --set "MAINTAINER=$MAINTAINER"
  --set "DEPENDS=$DEPENDS" --set "X86_64_SHA256=$X86_SHA" "${LICENSE_ARGS[@]}")
if [[ -n "$ARM_SHA" ]]; then
  RENDER_ARGS+=(--enable AARCH64 --set "AARCH64_SHA256=$ARM_SHA")
fi
python3 -I "$SCRIPT_DIR/release_render.py" template "$SCRIPT_DIR/clipsync-bin.PKGBUILD.in" \
  "$PKGDIR/PKGBUILD" "${RENDER_ARGS[@]}"

# Before publishing the release the source URLs do not exist yet, so give makepkg
# the staged files under their source names. A push lets makepkg download them.
if [[ "$MODE" == check ]]; then
  cp "$WORK/sources"/* "$PKGDIR/"
fi

CONTAINER=""
remove_container() {
  if [[ -n "$CONTAINER" ]]; then
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    CONTAINER=""
  fi
}
trap remove_container EXIT
clipsync_release_log "testing the package in $IMAGE"
CONTAINER="$(docker create --platform linux/amd64 \
  -e "CLIPSYNC_AUR_VERSION=$VERSION" \
  "$IMAGE" bash /work/aur-container-check.sh)" \
  || clipsync_release_die "could not create the Arch Linux container from $IMAGE"
docker cp "$WORK/ctx" "$CONTAINER:/work" >/dev/null \
  || clipsync_release_die "could not copy the package into the container"
docker start -a "$CONTAINER" || clipsync_release_die "the package failed its Arch Linux check; nothing was pushed"
docker cp "$CONTAINER:/work/pkg/.SRCINFO" "$PKGDIR/.SRCINFO" >/dev/null \
  || clipsync_release_die "the Arch Linux check wrote no .SRCINFO"
remove_container
trap - EXIT
grep -qxF "	pkgver = $VERSION" "$PKGDIR/.SRCINFO" \
  || clipsync_release_die ".SRCINFO is not for version $VERSION"

mkdir -p "$STAGE/aur"
cp "$PKGDIR/PKGBUILD" "$PKGDIR/.SRCINFO" "$STAGE/aur/"
clipsync_release_log "clipsync-bin $VERSION built, installed and passed namcap in Arch Linux"
clipsync_release_log "wrote $STAGE/aur/PKGBUILD and .SRCINFO"
if [[ "$MODE" == check ]]; then
  clipsync_release_log "check only; after the release is published run: just companion-release-aur"
  exit 0
fi

if [[ -n "${CLIPSYNC_AUR_SSH_KEY:-}" ]]; then
  [[ -f "$CLIPSYNC_AUR_SSH_KEY" ]] || clipsync_release_die "CLIPSYNC_AUR_SSH_KEY is not a file: $CLIPSYNC_AUR_SSH_KEY"
  export GIT_SSH_COMMAND="ssh -i '$CLIPSYNC_AUR_SSH_KEY' -o IdentitiesOnly=yes"
fi
git clone --quiet "$AUR_URL" "$WORK/aur" 2>"$WORK/clone.err" || {
  cat "$WORK/clone.err" >&2
  clipsync_release_die "could not clone $AUR_URL"
}
# A new AUR package is an empty repository; an existing one publishes master.
if git -C "$WORK/aur" rev-parse --quiet --verify refs/remotes/origin/master >/dev/null; then
  git -C "$WORK/aur" checkout --quiet -B master refs/remotes/origin/master
fi
if cmp -s "$PKGDIR/PKGBUILD" "$WORK/aur/PKGBUILD" && cmp -s "$PKGDIR/.SRCINFO" "$WORK/aur/.SRCINFO"; then
  clipsync_release_log "AUR already has clipsync-bin $VERSION; nothing to push"
  exit 0
fi
if [[ -f "$WORK/aur/.SRCINFO" ]] && grep -qxF "	pkgver = $VERSION" "$WORK/aur/.SRCINFO"; then
  clipsync_release_die "the AUR already has a different clipsync-bin $VERSION; a packaging-only change needs a pkgrel bump"
fi
cp "$PKGDIR/PKGBUILD" "$PKGDIR/.SRCINFO" "$WORK/aur/"
git -C "$WORK/aur" add PKGBUILD .SRCINFO
# The AUR shows commit authors publicly: use the maintainer identity, never the local git config.
GIT_AUTHOR_NAME="$AUR_NAME" GIT_AUTHOR_EMAIL="$AUR_EMAIL" \
  GIT_COMMITTER_NAME="$AUR_NAME" GIT_COMMITTER_EMAIL="$AUR_EMAIL" \
  git -C "$WORK/aur" -c commit.gpgsign=false commit --quiet -m "Update to $VERSION"
git -C "$WORK/aur" push --quiet origin HEAD:master \
  || clipsync_release_die "push to $AUR_URL failed; nothing was changed on the AUR"

clipsync_release_log "pushed clipsync-bin $VERSION to $AUR_URL"
clipsync_release_log "install with: yay -S clipsync-bin (or any AUR helper)"
