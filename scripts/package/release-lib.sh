#!/usr/bin/env bash
# Shared helpers for release-stage.sh, release-publish.sh and release-tap.sh.
# shellcheck shell=bash

set -euo pipefail

RELEASE_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$RELEASE_LIB_DIR/common.sh"

clipsync_release_die() {
  printf '[release] REFUSED: %s\n' "$*" >&2
  exit 1
}

clipsync_release_log() {
  printf '[release] %s\n' "$*"
}

clipsync_release_require_tools() {
  local tool
  for tool in "$@"; do
    command -v "$tool" >/dev/null 2>&1 || clipsync_release_die "required tool not found: $tool"
  done
}

# owner/repo of the GitHub project, from CLIPSYNC_RELEASE_REPO or the origin URL.
clipsync_release_repo_slug() {
  local root="$1" url slug
  if [[ -n "${CLIPSYNC_RELEASE_REPO:-}" ]]; then
    slug="$CLIPSYNC_RELEASE_REPO"
  else
    url="$(git -C "$root" remote get-url origin 2>/dev/null)" \
      || clipsync_release_die "no origin remote; set CLIPSYNC_RELEASE_REPO=owner/repo"
    slug="$(printf '%s' "$url" | sed -E -n 's#^(git@github\.com:|ssh://git@github\.com/|https://github\.com/)([^/]+/[^/]+)$#\2#p')"
    slug="${slug%.git}"
  fi
  [[ "$slug" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] \
    || clipsync_release_die "cannot determine the GitHub repository (got '${slug:-}'); set CLIPSYNC_RELEASE_REPO=owner/repo"
  printf '%s' "$slug"
}

clipsync_release_cargo_version() {
  local manifest="$1"
  [[ -f "$manifest" ]] || clipsync_release_die "missing $manifest"
  # First version key of the [package] table.
  awk '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^version[[:space:]]*=/{gsub(/.*=[[:space:]]*"|".*/, ""); print; exit}' "$manifest"
}

# Version shared by the root and desktop crates; refuses when they disagree.
clipsync_release_version() {
  local root="$1" core desktop
  core="$(clipsync_release_cargo_version "$root/Cargo.toml")"
  desktop="$(clipsync_release_cargo_version "$root/crates/desktop/Cargo.toml")"
  [[ -n "$core" && -n "$desktop" ]] || clipsync_release_die "could not read crate versions"
  [[ "$core" == "$desktop" ]] \
    || clipsync_release_die "crate versions disagree: Cargo.toml=$core crates/desktop/Cargo.toml=$desktop"
  [[ "$core" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] \
    || clipsync_release_die "version '$core' is not a release version (x.y.z)"
  printf '%s' "$core"
}

clipsync_release_stage_dir() {
  local root="$1" version="$2"
  printf '%s/v%s' "${CLIPSYNC_RELEASE_ROOT:-$root/dist/release}" "$version"
}

clipsync_release_sha256() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    sha256sum "$1" | awk '{print $1}'
  fi
}

clipsync_release_require_clean_tree() {
  local root="$1" status
  status="$(git -C "$root" status --porcelain --untracked-files=all)" \
    || clipsync_release_die "git status failed in $root"
  [[ -z "$status" ]] || clipsync_release_die "working tree has uncommitted or untracked changes:
$status"
}

clipsync_release_require_main_at_origin() {
  local root="$1" branch head remote
  branch="$(git -C "$root" symbolic-ref --quiet --short HEAD 2>/dev/null || printf 'detached HEAD')"
  [[ "$branch" == "main" ]] || clipsync_release_die "not on main (on $branch); releases are cut from main"
  git -C "$root" fetch --quiet origin main \
    || clipsync_release_die "git fetch origin main failed; cannot confirm HEAD matches origin/main"
  head="$(git -C "$root" rev-parse HEAD)"
  remote="$(git -C "$root" rev-parse refs/remotes/origin/main)"
  [[ "$head" == "$remote" ]] \
    || clipsync_release_die "HEAD ($head) does not match origin/main ($remote); pull or push first"
}

clipsync_release_require_tag_absent() {
  local root="$1" tag="$2" rc=0
  if git -C "$root" rev-parse --quiet --verify "refs/tags/$tag" >/dev/null; then
    clipsync_release_die "tag $tag already exists locally; bump the version in both Cargo.toml files"
  fi
  git -C "$root" ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1 || rc=$?
  case "$rc" in
    0) clipsync_release_die "tag $tag already exists on origin; bump the version in both Cargo.toml files" ;;
    2) ;;
    *) clipsync_release_die "could not list tags on origin (git ls-remote exit $rc)" ;;
  esac
}

# Draft releases have no tag yet, so check the release list (drafts included).
clipsync_release_require_release_absent() {
  local slug="$1" tag="$2" tags
  tags="$(gh release list --repo "$slug" --limit 1000 --json tagName --jq '.[].tagName')" \
    || clipsync_release_die "could not list GitHub releases for $slug (gh auth or network); refusing to guess"
  if grep -qxF "$tag" <<<"$tags"; then
    clipsync_release_die "a GitHub release (draft or published) for $tag already exists in $slug"
  fi
}

# Read one top-level key of a JSON file; prints nothing when missing.
clipsync_release_json_get() {
  python3 -I -c '
import json, sys
value = json.load(open(sys.argv[1])).get(sys.argv[2])
if value is None:
    sys.exit(0)
print(json.dumps(value) if isinstance(value, (dict, list, bool)) else value)
' "$1" "$2"
}
