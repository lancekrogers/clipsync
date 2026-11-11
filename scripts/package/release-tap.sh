#!/usr/bin/env bash
# Push the staged Homebrew cask to the tap once the GitHub release is public.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-lib.sh
source "$SCRIPT_DIR/release-lib.sh"

case "${1:-}" in
  "") ;;
  -h|--help)
    cat <<EOF
Usage: release-tap.sh

Commits dist/release/v<version>/clipsync.rb to Casks/clipsync.rb in the tap and
pushes it to the tap's main branch, matching how the tap is maintained.
Refuses unless the GitHub release is published (not a draft) and the public
disk image download has the staged checksum.

Environment:
  CLIPSYNC_TAP_REPO     default lancekrogers/homebrew-tap
  CLIPSYNC_TAP_URL      clone/push URL (default: same protocol as origin)
  CLIPSYNC_TAP_BRANCH   default main
EOF
    exit 0
    ;;
  *) clipsync_release_die "unknown argument: $1" ;;
esac

clipsync_release_require_tools git gh curl python3

ROOT="$(clipsync_package_project_root)"
VERSION="$(clipsync_release_version "$ROOT")"
TAG="v$VERSION"
SLUG="$(clipsync_release_repo_slug "$ROOT")"
STAGE="$(clipsync_release_stage_dir "$ROOT" "$VERSION")"
MANIFEST="$STAGE/release.json"
CASK="$STAGE/clipsync.rb"
TAP_REPO="${CLIPSYNC_TAP_REPO:-lancekrogers/homebrew-tap}"
TAP_BRANCH="${CLIPSYNC_TAP_BRANCH:-main}"

[[ -f "$MANIFEST" ]] || clipsync_release_die "incomplete staging directory (no release.json): $STAGE"
[[ -f "$CASK" ]] || clipsync_release_die "no staged cask at $CASK"

DMG_NAME="$(python3 -I -c '
import json, sys
names = [a["name"] for a in json.load(open(sys.argv[1]))["assets"] if a["name"].endswith(".dmg")]
print(names[0] if len(names) == 1 else "")' "$MANIFEST")"
DMG_SHA="$(python3 -I -c '
import json, sys
print(next(a["sha256"] for a in json.load(open(sys.argv[1]))["assets"] if a["name"] == sys.argv[2]))' "$MANIFEST" "$DMG_NAME" 2>/dev/null || true)"
[[ -n "$DMG_NAME" && -n "$DMG_SHA" ]] || clipsync_release_die "release.json does not list exactly one disk image"
[[ "$(clipsync_release_sha256 "$CASK")" == "$(clipsync_release_json_get "$MANIFEST" cask_sha256)" ]] \
  || clipsync_release_die "staged cask changed after staging (sha256 differs from release.json)"
grep -qF "version \"$VERSION\"" "$CASK" || clipsync_release_die "staged cask is not for version $VERSION"
grep -qF "sha256 \"$DMG_SHA\"" "$CASK" || clipsync_release_die "staged cask sha256 does not match the staged disk image"
COMMIT="$(clipsync_release_json_get "$MANIFEST" commit)"

DRAFT="$(gh release view "$TAG" --repo "$SLUG" --json isDraft --jq '.isDraft')" \
  || clipsync_release_die "no GitHub release $TAG in $SLUG (or GitHub is unreachable); publish the draft first"
[[ "$DRAFT" == "false" ]] \
  || clipsync_release_die "release $TAG is still a draft; publish it before updating the tap"

# The published tag must point at the staged commit.
TAG_REFS="$(git -C "$ROOT" ls-remote origin "refs/tags/$TAG" "refs/tags/$TAG^{}")" \
  || clipsync_release_die "could not list tags on origin"
TAG_SHA="$(awk 'END {print $1}' <<<"$TAG_REFS")"
[[ "$TAG_SHA" == "$COMMIT" ]] \
  || clipsync_release_die "tag $TAG on origin points at '${TAG_SHA:-nothing}', not the staged commit $COMMIT"

# Download and clone inside the stage so nothing needs cleaning up elsewhere.
mkdir -p "$STAGE/work"
WORK="$(mktemp -d "$STAGE/work/tap.XXXXXX")"
DMG_URL="https://github.com/$SLUG/releases/download/$TAG/$DMG_NAME"
clipsync_release_log "downloading $DMG_URL"
curl -fsSL --retry 3 -o "$WORK/$DMG_NAME" "$DMG_URL" \
  || clipsync_release_die "could not download the public disk image from $DMG_URL"
PUBLIC_SHA="$(clipsync_release_sha256 "$WORK/$DMG_NAME")"
[[ "$PUBLIC_SHA" == "$DMG_SHA" ]] \
  || clipsync_release_die "public disk image sha256 $PUBLIC_SHA does not match the staged $DMG_SHA"

if [[ -n "${CLIPSYNC_TAP_URL:-}" ]]; then
  TAP_URL="$CLIPSYNC_TAP_URL"
elif [[ "$(git -C "$ROOT" remote get-url origin 2>/dev/null)" == git@* ]]; then
  TAP_URL="git@github.com:$TAP_REPO.git"
else
  TAP_URL="https://github.com/$TAP_REPO.git"
fi

git clone --quiet --depth 1 --branch "$TAP_BRANCH" "$TAP_URL" "$WORK/tap" \
  || clipsync_release_die "could not clone $TAP_URL"
mkdir -p "$WORK/tap/Casks"
if [[ -f "$WORK/tap/Casks/clipsync.rb" ]] && cmp -s "$CASK" "$WORK/tap/Casks/clipsync.rb"; then
  clipsync_release_log "tap already has the clipsync $VERSION cask; nothing to push"
  exit 0
fi
cp "$CASK" "$WORK/tap/Casks/clipsync.rb"
git -C "$WORK/tap" add Casks/clipsync.rb
git -C "$WORK/tap" commit --quiet -m "Update clipsync cask to $TAG"
git -C "$WORK/tap" push --quiet origin "HEAD:$TAP_BRANCH" \
  || clipsync_release_die "push to $TAP_URL failed; nothing was changed in the tap"

TAP_NAME="${TAP_REPO%%/*}/${TAP_REPO#*/homebrew-}"
clipsync_release_log "pushed Casks/clipsync.rb ($TAG) to $TAP_REPO $TAP_BRANCH"
clipsync_release_log "install with: brew install --cask $TAP_NAME/clipsync"
