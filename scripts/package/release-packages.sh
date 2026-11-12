#!/usr/bin/env bash
# Update the Homebrew tap and the AUR from a published GitHub release. The
# Release packages workflow runs this when a release is published.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-lib.sh
source "$SCRIPT_DIR/release-lib.sh"

case "${1:-}" in
  -h|--help)
    cat <<EOF
Usage: release-packages.sh <tag>

Downloads the assets of the published release <tag>, including the release.json
and clipsync.rb that the draft step uploads, into dist/release/v<version>/, then
runs release-tap.sh and release-aur.sh against them. Both run even if the other
fails. The checkout must be at the version of <tag>.

Takes the environment of release-tap.sh and release-aur.sh.
EOF
    exit 0
    ;;
  "") clipsync_release_die "usage: release-packages.sh <tag>" ;;
esac
[[ $# -eq 1 ]] || clipsync_release_die "unexpected arguments: $*"
TAG="$1"

clipsync_release_require_tools git gh python3

ROOT="$(clipsync_package_project_root)"
VERSION="$(clipsync_release_version "$ROOT")"
SLUG="$(clipsync_release_repo_slug "$ROOT")"
[[ "$TAG" == "v$VERSION" ]] \
  || clipsync_release_die "$TAG is not the version of this checkout (v$VERSION); check out $TAG first"
STAGE="$(clipsync_release_stage_dir "$ROOT" "$VERSION")"
[[ ! -e "$STAGE" ]] \
  || clipsync_release_die "$STAGE already exists; for a local stage run just companion-release-tap and just companion-release-aur"

mkdir -p "$STAGE/assets"
gh release download "$TAG" --repo "$SLUG" --dir "$STAGE/assets" \
  || clipsync_release_die "could not download the assets of release $TAG"
for file in release.json clipsync.rb; do
  [[ -f "$STAGE/assets/$file" ]] \
    || clipsync_release_die "release $TAG has no $file asset; run just companion-release-tap and just companion-release-aur from its stage instead"
  mv "$STAGE/assets/$file" "$STAGE/$file"
done
[[ "$(clipsync_release_json_get "$STAGE/release.json" tag)" == "$TAG" ]] \
  || clipsync_release_die "the release.json attached to $TAG is for another tag"

failed=()
bash "$SCRIPT_DIR/release-tap.sh" || failed+=("Homebrew tap")
bash "$SCRIPT_DIR/release-aur.sh" || failed+=("AUR")
[[ ${#failed[@]} -eq 0 ]] || clipsync_release_die "not updated: ${failed[*]}"
clipsync_release_log "Homebrew tap and AUR are up to date for $TAG"
