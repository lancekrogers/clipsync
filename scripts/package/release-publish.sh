#!/usr/bin/env bash
# Upload a staged release (release-stage.sh) as a DRAFT GitHub release.
# Publishing the draft, which creates the tag, stays a manual step; it starts
# the Release packages workflow, which updates the Homebrew tap and the AUR.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-lib.sh
source "$SCRIPT_DIR/release-lib.sh"

case "${1:-}" in
  "") ;;
  -h|--help)
    cat <<EOF
Usage: release-publish.sh

Creates a draft GitHub release for the version staged in dist/release/v<version>.
Refuses unless the staged files still match release.json and SHA256SUMS, quality
gates ran, HEAD is the staged commit, and the tag and release do not exist yet.
EOF
    exit 0
    ;;
  *) clipsync_release_die "unknown argument: $1" ;;
esac

clipsync_release_require_tools git gh python3

ROOT="$(clipsync_package_project_root)"
VERSION="$(clipsync_release_version "$ROOT")"
TAG="v$VERSION"
SLUG="$(clipsync_release_repo_slug "$ROOT")"
STAGE="$(clipsync_release_stage_dir "$ROOT" "$VERSION")"
MANIFEST="$STAGE/release.json"

[[ -d "$STAGE" ]] || clipsync_release_die "nothing staged for $TAG at $STAGE; run just companion-release first"
[[ -f "$MANIFEST" ]] || clipsync_release_die "incomplete staging directory (no release.json): $STAGE; staging did not finish, so restage"

# Validate the stage against release.json: field values, the asset set, every
# checksum, and SHA256SUMS. Prints the asset names in upload order.
ASSETS_LIST="$(python3 -I - "$STAGE" "$VERSION" "$TAG" "$SLUG" <<'PY'
import hashlib, json, re, sys
from pathlib import Path

stage, version, tag, slug = Path(sys.argv[1]), *sys.argv[2:]
problems = []
try:
    data = json.loads((stage / "release.json").read_text())
except Exception as exc:
    sys.exit(f"release.json is unreadable: {exc}")

def expect(key, want):
    if data.get(key) != want:
        problems.append(f"release.json {key}={data.get(key)!r}, expected {want!r}")

expect("version", version)
expect("tag", tag)
expect("repo", slug)
expect("release_build", True)
expect("fixture_mode", False)
if data.get("quality_gates") != "passed":
    problems.append(f"quality gates were {data.get('quality_gates')!r} when staged; restage without --skip-gates")
if data.get("signing") not in ("unsigned", "developer-id"):
    problems.append(f"unknown signing {data.get('signing')!r}")
if not re.fullmatch(r"[0-9a-f]{40}", str(data.get("commit", ""))):
    problems.append("release.json has no full commit SHA")

def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()

assets = data.get("assets") or []
names = [a.get("name") for a in assets]
assets_dir = stage / "assets"
if "SHA256SUMS" not in names or not any(n and n.endswith(".dmg") for n in names) \
        or not any(n and n.endswith(".tar.gz") for n in names):
    problems.append("release.json must list a disk image, a Linux archive and SHA256SUMS")
on_disk = sorted(p.name for p in assets_dir.iterdir()) if assets_dir.is_dir() else []
if sorted(n for n in names if n) != on_disk:
    problems.append(f"assets/ holds {on_disk}, release.json lists {sorted(n for n in names if n)}")
for asset in assets:
    path = assets_dir / str(asset.get("name"))
    if not path.is_file():
        problems.append(f"missing asset {asset.get('name')}")
    elif sha256(path) != asset.get("sha256"):
        problems.append(f"checksum mismatch for {asset.get('name')} (changed after staging)")

sums_path = assets_dir / "SHA256SUMS"
if sums_path.is_file():
    listed = {}
    for line in sums_path.read_text().splitlines():
        parts = line.split()
        if len(parts) != 2:
            problems.append(f"malformed SHA256SUMS line: {line!r}")
            continue
        listed[parts[1].lstrip("*")] = parts[0]
    expected = {a["name"]: a["sha256"] for a in assets if a.get("name") not in (None, "SHA256SUMS")}
    if listed != expected:
        problems.append("SHA256SUMS does not match the staged files")

cask = stage / "clipsync.rb"
if not cask.is_file():
    problems.append("clipsync.rb (the Homebrew cask) is missing")
elif sha256(cask) != data.get("cask_sha256"):
    problems.append("clipsync.rb changed after staging")

notes = stage / str(data.get("notes", "RELEASE_NOTES.md"))
if not notes.is_file() or not notes.read_text().strip():
    problems.append("RELEASE_NOTES.md is missing or empty")
elif re.search(r"@[A-Z][A-Z0-9_]*@", notes.read_text()):
    problems.append("RELEASE_NOTES.md has unfilled placeholders")

if problems:
    sys.exit("\n".join(problems))
for name in names:
    print(name)
PY
)" || clipsync_release_die "staged release does not validate (see above)"

COMMIT="$(clipsync_release_json_get "$MANIFEST" commit)"
HEAD="$(git -C "$ROOT" rev-parse HEAD)"
[[ "$HEAD" == "$COMMIT" ]] \
  || clipsync_release_die "HEAD ($HEAD) is not the staged commit ($COMMIT); restage or check out the staged commit"
git -C "$ROOT" fetch --quiet origin main \
  || clipsync_release_die "git fetch origin main failed; cannot confirm the staged commit is on origin"
git -C "$ROOT" merge-base --is-ancestor "$COMMIT" refs/remotes/origin/main \
  || clipsync_release_die "staged commit $COMMIT is not on origin/main"
clipsync_release_require_tag_absent "$ROOT" "$TAG"
clipsync_release_require_release_absent "$SLUG" "$TAG"

UPLOADS=()
while IFS= read -r name; do
  [[ -n "$name" ]] && UPLOADS+=("$STAGE/assets/$name")
done <<<"$ASSETS_LIST"
# The Release packages workflow builds the cask and AUR updates from these two.
UPLOADS+=("$STAGE/clipsync.rb" "$MANIFEST")

clipsync_release_log "creating DRAFT release $TAG in $SLUG at $COMMIT"
URL="$(gh release create "$TAG" \
  --repo "$SLUG" \
  --draft \
  --target "$COMMIT" \
  --title "ClipSync $VERSION" \
  --notes-file "$STAGE/RELEASE_NOTES.md" \
  "${UPLOADS[@]}")" || clipsync_release_die "gh release create failed; check the Releases page for a partial draft before retrying"

clipsync_release_log "draft created: $URL"
cat <<EOF
[release] Nothing is public yet: the tag $TAG does not exist until the draft is published.
[release] Review the draft, then publish it with either:
[release]   gh release edit $TAG --repo $SLUG --draft=false
[release]   or "Publish release" on $URL
[release] Publishing starts the Release packages workflow, which updates the Homebrew
[release] cask and the AUR package. Check it with: gh run list --workflow release-packages.yml
EOF
