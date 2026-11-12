#!/usr/bin/env bash
# Isolated tests for release-stage.sh, release-publish.sh, release-tap.sh and
# release-aur.sh.
# A throwaway git repo with a local bare origin, isolated git config, and stub
# gh/cargo/docker/curl/codesign/hdiutil on PATH: nothing reaches GitHub and
# nothing is built.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

TMP_BASE="${TMPDIR:-/tmp}"
TMP_BASE="${TMP_BASE%/}"
TMP="$(mktemp -d "$TMP_BASE/clipsync-release-test.XXXXXX")"
cleanup() {
  [[ "$TMP" == "$TMP_BASE"/clipsync-release-test.?????? && -d "$TMP" ]] && rm -rf -- "$TMP"
  return 0
}
trap cleanup EXIT

PASS=0
FAIL=0
pass() { echo "PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

# Release scripts and git must not see the developer's real config or credentials.
unset CLIPSYNC_CODESIGN_IDENTITY CLIPSYNC_NOTARY_PROFILE CLIPSYNC_NOTARY_APPLE_ID \
  CLIPSYNC_NOTARY_TEAM_ID CLIPSYNC_NOTARY_PASSWORD CLIPSYNC_PACKAGE_FIXTURE_MODE \
  CLIPSYNC_PACKAGE_SKIP_BUILD CLIPSYNC_PACKAGE_CLI_BIN CLIPSYNC_PACKAGE_GUI_BIN \
  CLIPSYNC_PACKAGE_FORCE CLIPSYNC_PACKAGE_DIST CLIPSYNC_RELEASE_ROOT CLIPSYNC_TAP_URL \
  CLIPSYNC_TAP_REPO CLIPSYNC_RELEASE_LINUX_ARCHES CLIPSYNC_RUSTUP_TOOLCHAIN \
  CLIPSYNC_AUR_URL CLIPSYNC_AUR_SSH_KEY CLIPSYNC_AUR_MAINTAINER CLIPSYNC_AUR_IMAGE
cat >"$TMP/gitconfig" <<EOF
[user]
	name = Release Test
	email = release-test@example.invalid
[commit]
	gpgsign = false
[tag]
	gpgsign = false
[init]
	defaultBranch = main
[advice]
	detachedHead = false
EOF
export GIT_CONFIG_GLOBAL="$TMP/gitconfig" GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0
export CLIPSYNC_RELEASE_REPO=example/clipsync

STUBS="$TMP/stubs"
LOGS="$TMP/logs"
mkdir -p "$STUBS" "$LOGS"
export STUB_LOGS="$LOGS"

cat >"$STUBS/gh" <<'EOS'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$STUB_LOGS/gh.log"
case "${1:-} ${2:-}" in
  "release list")
    if [[ "${STUB_GH_LIST_FAIL:-0}" == 1 ]]; then echo "HTTP 401: Bad credentials" >&2; exit 1; fi
    for tag in ${STUB_GH_RELEASES:-}; do echo "$tag"; done
    exit 0 ;;
  "release view")
    if [[ -z "${STUB_GH_VIEW_DRAFT:-}" ]]; then echo "release not found" >&2; exit 1; fi
    echo "$STUB_GH_VIEW_DRAFT"
    exit 0 ;;
  "release create")
    printf '%s\n' "$@" >"$STUB_LOGS/gh-create.args"
    echo "https://github.com/example/clipsync/releases/tag/untagged-0123"
    exit 0 ;;
esac
echo "unexpected gh call: $*" >&2
exit 99
EOS

cat >"$STUBS/cargo" <<'EOS'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$STUB_LOGS/cargo.log"
[[ " $* " == *" ${STUB_CARGO_FAIL:-none} "* ]] && exit 1
exit 0
EOS

cat >"$STUBS/curl" <<'EOS'
#!/usr/bin/env bash
out="" url=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
printf '%s\n' "$url" >>"$STUB_LOGS/curl.log"
[[ "${STUB_CURL_FAIL:-0}" == 1 ]] && exit 22
if [[ -n "${STUB_CURL_DIR:-}" ]]; then
  cp "$STUB_CURL_DIR/${url##*/}" "$out"
else
  cp "$STUB_CURL_SOURCE" "$out"
fi
EOS

# docker refuses unless STUB_DOCKER_AUR=1, which fakes the Arch Linux check
# container: each create gets its own directory, and start writes a .SRCINFO.
cat >"$STUBS/docker" <<'EOS'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$STUB_LOGS/docker.log"
if [[ "${STUB_DOCKER_AUR:-0}" != 1 ]]; then
  echo "docker must not be reached in release tests" >&2
  exit 97
fi
box_of() { printf '%s/container-%s' "$STUB_LOGS" "${1#aur-check-}"; }
case "${1:-}" in
  create)
    n=$(($(cat "$STUB_LOGS/docker.count" 2>/dev/null || echo 0) + 1))
    echo "$n" >"$STUB_LOGS/docker.count"
    mkdir -p "$(box_of "$n")"
    echo "aur-check-$n" ;;
  cp)
    if [[ "$2" == aur-check-*:* ]]; then
      cp "$(box_of "${2%%:*}")${2#*:}" "$3"
    else
      cp -R "$2" "$(box_of "${3%%:*}")${3#*:}"
    fi ;;
  start)
    box="$(box_of "$3")"
    if [[ "${STUB_DOCKER_START_FAIL:-0}" == 1 ]]; then echo "[aur-check] FAILED: stub" >&2; exit 1; fi
    pkgver="$(sed -n 's/^pkgver=//p' "$box/work/pkg/PKGBUILD")"
    printf 'pkgbase = clipsync-bin\n\tpkgver = %s\n\tpkgrel = 1\n' "$pkgver" >"$box/work/pkg/.SRCINFO"
    cksum <"$box/work/pkg/PKGBUILD" >>"$box/work/pkg/.SRCINFO" ;;
  rm) ;;
  *) echo "unexpected docker call: $*" >&2; exit 99 ;;
esac
EOS
for tool in codesign hdiutil; do
  cat >"$STUBS/$tool" <<EOS
#!/usr/bin/env bash
printf '%s\n' "\$*" >>"\$STUB_LOGS/$tool.log"
echo "$tool must not be reached in release tests" >&2
exit 97
EOS
done
chmod 755 "$STUBS"/*
REAL_PATH="$PATH"
export PATH="$STUBS:$PATH"

# --- fixture repository with a local bare origin ---
ORIGIN="$TMP/origin.git"
REPO="$TMP/repo"
OTHER="$TMP/other"
git init -q --bare "$ORIGIN"
git init -q "$REPO"
mkdir -p "$REPO/scripts/package" "$REPO/crates/desktop"
cp "$SCRIPT_DIR"/*.sh "$SCRIPT_DIR"/*.py "$SCRIPT_DIR"/*.in "$SCRIPT_DIR/Dockerfile.release-linux" \
  "$REPO/scripts/package/"
cat >"$REPO/Cargo.toml" <<'EOF'
[workspace]
members = [".", "crates/desktop"]

[package]
name = "clipsync"
version = "0.1.0"
EOF
cat >"$REPO/crates/desktop/Cargo.toml" <<'EOF'
[package]
name = "clipsync-desktop"
version = "0.1.0"
EOF
printf 'dist/\ntarget/\n' >"$REPO/.gitignore"
printf 'MIT fixture license\n' >"$REPO/LICENSE-MIT"
printf 'Apache fixture license\n' >"$REPO/LICENSE-APACHE"
git -C "$REPO" add -A
git -C "$REPO" commit -q -m "fixture"
git -C "$REPO" remote add origin "$ORIGIN"
git -C "$REPO" push -q origin main
git -C "$REPO" fetch -q origin
git clone -q "$ORIGIN" "$OTHER"

STAGE_SH="$REPO/scripts/package/release-stage.sh"
PUBLISH_SH="$REPO/scripts/package/release-publish.sh"
TAP_SH="$REPO/scripts/package/release-tap.sh"
RENDER="$REPO/scripts/package/release_render.py"

commit_and_push() {
  local message="$1"
  git -C "$REPO" commit -q --allow-empty -m "$message"
  git -C "$REPO" push -q origin main
}

# expect_refusal NAME NEEDLE CMD...: non-zero exit and NEEDLE in the output.
expect_refusal() {
  local name="$1" needle="$2" out rc=0
  shift 2
  out="$("$@" 2>&1)" || rc=$?
  if [[ "$rc" -ne 0 && "$out" == *"$needle"* ]]; then
    pass "$name"
  else
    fail "$name (exit $rc; wanted '$needle')"
    printf '%s\n' "$out" | tail -8 | sed 's/^/    /' >&2
  fi
}

expect_success() {
  local name="$1" out rc=0
  shift
  out="$("$@" 2>&1)" || rc=$?
  LAST_OUT="$out"
  if [[ "$rc" -eq 0 ]]; then
    pass "$name"
  else
    fail "$name (exit $rc)"
    printf '%s\n' "$out" | tail -12 | sed 's/^/    /' >&2
  fi
}

# ===================== stage preflight =====================
if [[ "$(uname -s)" == "Darwin" && "$(uname -m)" == "arm64" ]]; then
  export CLIPSYNC_RELEASE_ROOT="$TMP/stage-root"

  expect_refusal "stage refuses fixture mode" "fixture or prebuilt binaries cannot be staged" \
    env CLIPSYNC_PACKAGE_FIXTURE_MODE=1 bash "$STAGE_SH"
  expect_refusal "stage refuses prebuilt binaries" "fixture or prebuilt binaries cannot be staged" \
    env CLIPSYNC_PACKAGE_CLI_BIN=/bin/ls bash "$STAGE_SH"
  expect_refusal "stage refuses a prebuilt GUI" "fixture or prebuilt binaries cannot be staged" \
    env CLIPSYNC_PACKAGE_GUI_BIN=/bin/ls bash "$STAGE_SH"
  expect_refusal "stage refuses skipping the build" "fixture or prebuilt binaries cannot be staged" \
    env CLIPSYNC_PACKAGE_SKIP_BUILD=1 bash "$STAGE_SH"
  for override in CLIPSYNC_PACKAGE_MOCK_BIN=/tmp CARGO_BUILD_TARGET=x86_64-apple-darwin \
    RUSTFLAGS=-Ctarget-cpu=native CARGO_ENCODED_RUSTFLAGS=-Copt-level=0 CARGO_BUILD_RUSTFLAGS=-g; do
    expect_refusal "stage refuses ${override%%=*}" "unset ${override%%=*}: release builds take no" \
      env "$override" bash "$STAGE_SH"
  done
  expect_refusal "stage refuses a floating toolchain channel" "need an exact toolchain version" \
    env CLIPSYNC_RUSTUP_TOOLCHAIN=stable bash "$STAGE_SH"
  expect_refusal "stage refuses a Linux set without x86_64" "x86_64 Linux archive is required" \
    env CLIPSYNC_RELEASE_LINUX_ARCHES=aarch64 bash "$STAGE_SH"

  echo "# edit" >>"$REPO/Cargo.toml"
  expect_refusal "stage refuses a modified tracked file" "uncommitted or untracked" bash "$STAGE_SH"
  git -C "$REPO" checkout -q -- Cargo.toml

  touch "$REPO/untracked.txt"
  expect_refusal "stage refuses an untracked file" "uncommitted or untracked" bash "$STAGE_SH"
  rm -f "$REPO/untracked.txt"

  git -C "$REPO" update-index --skip-worktree Cargo.toml
  echo "# hidden" >>"$REPO/Cargo.toml"
  expect_refusal "stage refuses skip-worktree files" "assume-unchanged or skip-worktree" bash "$STAGE_SH"
  git -C "$REPO" update-index --no-skip-worktree Cargo.toml
  git -C "$REPO" checkout -q -- Cargo.toml
  git -C "$REPO" update-index --assume-unchanged .gitignore
  expect_refusal "stage refuses assume-unchanged files" "assume-unchanged or skip-worktree" bash "$STAGE_SH"
  git -C "$REPO" update-index --no-assume-unchanged .gitignore

  git -C "$REPO" checkout -q -b feature
  expect_refusal "stage refuses a branch other than main" "not on main (on feature)" bash "$STAGE_SH"
  git -C "$REPO" checkout -q main
  git -C "$REPO" branch -q -D feature

  git -C "$OTHER" pull -q --ff-only
  git -C "$OTHER" commit -q --allow-empty -m "ahead on origin"
  git -C "$OTHER" push -q origin main
  expect_refusal "stage refuses HEAD behind origin/main" "does not match origin/main" bash "$STAGE_SH"
  git -C "$REPO" pull -q --ff-only origin main

  git -C "$REPO" commit -q --allow-empty -m "local only"
  expect_refusal "stage refuses HEAD ahead of origin/main" "does not match origin/main" bash "$STAGE_SH"
  git -C "$REPO" push -q origin main

  sed -i.bak 's/^version = "0.1.0"/version = "0.2.0"/' "$REPO/crates/desktop/Cargo.toml"
  rm -f "$REPO/crates/desktop/Cargo.toml.bak"
  git -C "$REPO" commit -q -am "desktop 0.2.0"
  git -C "$REPO" push -q origin main
  expect_refusal "stage refuses mismatched crate versions" \
    "crate versions disagree: Cargo.toml=0.1.0 crates/desktop/Cargo.toml=0.2.0" bash "$STAGE_SH"
  git -C "$REPO" revert --no-edit HEAD >/dev/null
  git -C "$REPO" push -q origin main

  sed -i.bak 's/^version = "0.1.0"/version = "0.1.0-rc1"/' "$REPO/Cargo.toml" "$REPO/crates/desktop/Cargo.toml"
  rm -f "$REPO/Cargo.toml.bak" "$REPO/crates/desktop/Cargo.toml.bak"
  git -C "$REPO" commit -q -am "prerelease"
  git -C "$REPO" push -q origin main
  expect_refusal "stage refuses a prerelease version" "is not a release version (x.y.z)" bash "$STAGE_SH"
  git -C "$REPO" revert --no-edit HEAD >/dev/null
  git -C "$REPO" push -q origin main

  git -C "$REPO" tag v0.1.0
  expect_refusal "stage refuses an existing local tag" "tag v0.1.0 already exists locally" bash "$STAGE_SH"
  git -C "$REPO" push -q origin v0.1.0
  git -C "$REPO" tag -d v0.1.0 >/dev/null
  expect_refusal "stage refuses a tag that exists on origin" "tag v0.1.0 already exists on origin" bash "$STAGE_SH"
  git -C "$REPO" push -q origin :refs/tags/v0.1.0

  expect_refusal "stage refuses an existing GitHub release or draft" \
    "a GitHub release (draft or published) for v0.1.0 already exists" \
    env STUB_GH_RELEASES="v0.0.9 v0.1.0" bash "$STAGE_SH"
  expect_refusal "stage fails closed when releases cannot be listed" \
    "could not list GitHub releases" env STUB_GH_LIST_FAIL=1 bash "$STAGE_SH"

  mkdir -p "$CLIPSYNC_RELEASE_ROOT/v0.1.0"
  expect_refusal "stage refuses an existing staging directory" "staging directory already exists" bash "$STAGE_SH"
  rmdir "$CLIPSYNC_RELEASE_ROOT/v0.1.0"

  expect_refusal "stage refuses an identity without notary credentials" \
    "notarization credentials are incomplete" \
    env CLIPSYNC_CODESIGN_IDENTITY="Developer ID Application: Test" bash "$STAGE_SH"
  expect_refusal "stage refuses notary credentials without an identity" "set both or neither" \
    env CLIPSYNC_NOTARY_PROFILE=test-profile bash "$STAGE_SH"
  expect_refusal "stage refuses an ad-hoc signing identity" "is ad-hoc signing" \
    env CLIPSYNC_CODESIGN_IDENTITY=- CLIPSYNC_NOTARY_PROFILE=test-profile bash "$STAGE_SH"

  for gate in fmt test; do
    expect_refusal "stage stops when the $gate gate fails" "quality gate failed: $gate" \
      env STUB_CARGO_FAIL="$gate" bash "$STAGE_SH"
  done

  : >"$LOGS/cargo.log"
  expect_refusal "stage stops when a quality gate fails" "quality gate failed: clippy" \
    env STUB_CARGO_FAIL=clippy bash "$STAGE_SH"
  if [[ "$(cat "$LOGS/cargo.log")" == "+1.95.0 fmt --all -- --check
+1.95.0 clippy --workspace --all-features -- -D warnings" ]]; then
    pass "gates run fmt then clippy with the documented arguments, and stop before tests"
  else
    fail "unexpected gate commands: $(tr '\n' '|' <"$LOGS/cargo.log")"
  fi
  if [[ ! -e "$CLIPSYNC_RELEASE_ROOT/v0.1.0" ]]; then
    pass "failed gate leaves no staging directory"
  else
    fail "failed gate created a staging directory"
  fi
  if [[ ! -s "$LOGS/docker.log" && ! -s "$LOGS/codesign.log" && ! -s "$LOGS/hdiutil.log" ]]; then
    pass "refused stages never reached a build"
  else
    fail "a refused stage reached docker/codesign/hdiutil"
  fi
  unset CLIPSYNC_RELEASE_ROOT
else
  echo "INFO: stage preflight tests need an arm64 Mac; skipped on $(uname -s) $(uname -m)"
fi

# ===================== staged-directory fixtures =====================
DMG="ClipSync-0.1.0-macos-arm64-unsigned.dmg"
X86="clipsync-0.1.0-linux-x86_64.tar.gz"
ARM="clipsync-0.1.0-linux-aarch64.tar.gz"
STAGE_N=0

# make_stage [gates] [fixture_mode] [linux arches]: fresh staged release for the current HEAD.
make_stage() {
  local gates="${1:-passed}" fixture="${2:-false}" arches="${3:-x86_64 aarch64}" stage arch
  local names=("$DMG") linux=()
  STAGE_N=$((STAGE_N + 1))
  export CLIPSYNC_RELEASE_ROOT="$TMP/stages/$STAGE_N"
  stage="$CLIPSYNC_RELEASE_ROOT/v0.1.0"
  mkdir -p "$stage/assets"
  printf 'fake disk image %s\n' "$STAGE_N" >"$stage/assets/$DMG"
  for arch in $arches; do
    printf 'fake %s archive\n' "$arch" >"$stage/assets/clipsync-0.1.0-linux-$arch.tar.gz"
    names+=("clipsync-0.1.0-linux-$arch.tar.gz")
    linux+=(--asset "clipsync-0.1.0-linux-$arch.tar.gz=linux:$arch")
  done
  (cd "$stage/assets" && shasum -a 256 "${names[@]}" >SHA256SUMS)
  python3 -I "$RENDER" template "$REPO/scripts/package/clipsync.rb.in" "$stage/clipsync.rb" \
    --set VERSION=0.1.0 --set "DMG_SHA256=$(shasum -a 256 "$stage/assets/$DMG" | awk '{print $1}')" \
    --set REPO=example/clipsync --set 'DMG_URL_NAME=ClipSync-#{version}-macos-arm64-unsigned.dmg' \
    --enable UNSIGNED
  printf 'ClipSync 0.1.0 notes\n' >"$stage/RELEASE_NOTES.md"
  python3 -I "$RENDER" manifest "$stage/release.json" \
    --field version=0.1.0 --field tag=v0.1.0 --field repo=example/clipsync \
    --field "commit=$(git -C "$REPO" rev-parse HEAD)" --field signing=unsigned \
    --field "quality_gates=$gates" --field rust_toolchain=1.95.0 \
    --field notes=RELEASE_NOTES.md --field cask=clipsync.rb \
    --field "cask_sha256=$(shasum -a 256 "$stage/clipsync.rb" | awk '{print $1}')" \
    --json-field release_build=true --json-field "fixture_mode=$fixture" --json-field notarized=false \
    --asset "$DMG=macos:arm64" "${linux[@]}" --asset "SHA256SUMS=any:any"
  STAGE="$stage"
}

# ===================== renderer and cask =====================
make_stage
if ! command -v ruby >/dev/null 2>&1; then
  echo "INFO: ruby not installed; cask syntax check skipped"
elif ruby -c "$STAGE/clipsync.rb" >/dev/null 2>&1; then
  pass "rendered cask is valid Ruby"
else
  fail "rendered cask is not valid Ruby"
fi
CASK_TEXT="$(cat "$STAGE/clipsync.rb")"
for needle in 'depends_on arch: :arm64' 'app "ClipSync.app"' \
  'binary "#{appdir}/ClipSync.app/Contents/MacOS/clipsync"' \
  'run "/usr/bin/xattr", args: ["-dr", "com.apple.quarantine", "{{appdir}}/ClipSync.app"]' \
  'url "https://github.com/example/clipsync/releases/download/v#{version}/ClipSync-#{version}-macos-arm64-unsigned.dmg"' \
  'uninstall quit: "com.clipsync.desktop"'; do
  if [[ "$CASK_TEXT" == *"$needle"* ]]; then pass "cask has: $needle"; else fail "cask lacks: $needle"; fi
done
if grep -A1 '^  uninstall' "$STAGE/clipsync.rb" | grep -q 'config\|local/share\|Application Support'; then
  fail "cask uninstall touches user data"
else
  pass "cask uninstall leaves user data to zap"
fi
python3 -I "$RENDER" template "$REPO/scripts/package/clipsync.rb.in" "$TMP/signed.rb" \
  --set VERSION=0.1.0 --set DMG_SHA256=abc --set REPO=example/clipsync \
  --set 'DMG_URL_NAME=ClipSync-#{version}-macos-arm64.dmg' --enable SIGNED
if grep -q quarantine "$TMP/signed.rb"; then
  fail "signed cask must not strip quarantine"
else
  pass "signed cask keeps Gatekeeper checks"
fi
expect_refusal "renderer refuses unfilled placeholders" "unfilled placeholders" \
  python3 -I "$RENDER" template "$REPO/scripts/package/clipsync.rb.in" "$TMP/partial.rb" --set VERSION=0.1.0

# ===================== publish =====================
make_stage
: >"$LOGS/gh.log"
rm -f "$LOGS/gh-create.args"
expect_success "publish creates a draft from a valid stage" bash "$PUBLISH_SH"
HEAD_SHA="$(git -C "$REPO" rev-parse HEAD)"
EXPECTED_ARGS="release
create
v0.1.0
--repo
example/clipsync
--draft
--target
$HEAD_SHA
--title
ClipSync 0.1.0
--notes-file
$STAGE/RELEASE_NOTES.md
$STAGE/assets/$DMG
$STAGE/assets/$X86
$STAGE/assets/$ARM
$STAGE/assets/SHA256SUMS"
if [[ -f "$LOGS/gh-create.args" && "$(cat "$LOGS/gh-create.args")" == "$EXPECTED_ARGS" ]]; then
  pass "gh release create gets --draft, --target <staged sha> and exactly the staged assets"
else
  fail "unexpected gh release create arguments"
  diff <(printf '%s\n' "$EXPECTED_ARGS") "$LOGS/gh-create.args" >&2 || true
fi
if [[ "$LAST_OUT" == *"draft created: https://github.com/example/clipsync/releases/tag/untagged-0123"* \
   && "$LAST_OUT" == *"gh release edit v0.1.0 --repo example/clipsync --draft=false"* ]]; then
  pass "publish prints the draft URL and the command that makes it public"
else
  fail "publish output lacks the draft URL or publish command"
fi
if [[ -z "$(git -C "$REPO" tag)" && -z "$(git -C "$REPO" ls-remote --tags origin)" ]]; then
  pass "publish creates no tag locally or on origin"
else
  fail "publish created a tag"
fi

publish_refusal() {
  local name="$1" needle="$2"
  shift 2
  rm -f "$LOGS/gh-create.args"
  expect_refusal "$name" "$needle" "$@"
  if [[ -e "$LOGS/gh-create.args" ]]; then
    fail "$name: gh release create was called"
  fi
}

make_stage
printf 'tampered' >>"$STAGE/assets/$X86"
publish_refusal "publish refuses an asset changed after staging" "checksum mismatch for $X86" bash "$PUBLISH_SH"

make_stage
rm -f "$STAGE/release.json"
publish_refusal "publish refuses a stage without release.json" "incomplete staging directory" bash "$PUBLISH_SH"

make_stage
rm -f "$STAGE/assets/$ARM"
publish_refusal "publish refuses a missing asset" "missing asset $ARM" bash "$PUBLISH_SH"

make_stage
printf 'stray\n' >"$STAGE/assets/notes.txt"
publish_refusal "publish refuses files not listed in release.json" "assets/ holds" bash "$PUBLISH_SH"

make_stage
(cd "$STAGE/assets" && shasum -a 256 "$DMG" "$X86" >SHA256SUMS)
publish_refusal "publish refuses a SHA256SUMS that no longer matches" "SHA256SUMS does not match the staged files" \
  bash "$PUBLISH_SH"

make_stage
printf 'Install @VERSION@\n' >"$STAGE/RELEASE_NOTES.md"
publish_refusal "publish refuses notes with unfilled placeholders" "unfilled placeholders" bash "$PUBLISH_SH"

make_stage skipped
publish_refusal "publish refuses a stage made with --skip-gates" "quality gates were 'skipped'" bash "$PUBLISH_SH"

make_stage passed true
publish_refusal "publish refuses a fixture-mode stage" "fixture_mode=True" bash "$PUBLISH_SH"

STAGE_N=$((STAGE_N + 1))
export CLIPSYNC_RELEASE_ROOT="$TMP/stages/$STAGE_N"
publish_refusal "publish refuses when nothing is staged" "nothing staged for v0.1.0" bash "$PUBLISH_SH"

make_stage
commit_and_push "moved on"
publish_refusal "publish refuses when HEAD is not the staged commit" "is not the staged commit" bash "$PUBLISH_SH"

git -C "$REPO" commit -q --allow-empty -m "unpushed"
make_stage
publish_refusal "publish refuses a staged commit that is not on origin/main" "is not on origin/main" bash "$PUBLISH_SH"
git -C "$REPO" push -q origin main

make_stage
git -C "$REPO" tag v0.1.0
publish_refusal "publish refuses an existing local tag" "tag v0.1.0 already exists locally" bash "$PUBLISH_SH"
git -C "$REPO" push -q origin v0.1.0
git -C "$REPO" tag -d v0.1.0 >/dev/null
publish_refusal "publish refuses a tag on origin" "tag v0.1.0 already exists on origin" bash "$PUBLISH_SH"
git -C "$REPO" push -q origin :refs/tags/v0.1.0

make_stage
rm -f "$STAGE/clipsync.rb"
publish_refusal "publish refuses a stage without the cask" "clipsync.rb (the Homebrew cask) is missing" bash "$PUBLISH_SH"

make_stage
printf '  # edited\n' >>"$STAGE/clipsync.rb"
publish_refusal "publish refuses a cask changed after staging" "clipsync.rb changed after staging" bash "$PUBLISH_SH"

make_stage
publish_refusal "publish refuses an existing release or draft" "already exists in example/clipsync" \
  env STUB_GH_RELEASES=v0.1.0 bash "$PUBLISH_SH"

# ===================== tap =====================
TAP="$TMP/tap.git"
TAP_SEED="$TMP/tap-seed"
git init -q --bare "$TAP"
git init -q "$TAP_SEED"
mkdir -p "$TAP_SEED/Formula"
printf '# formulae\n' >"$TAP_SEED/Formula/README.md"
git -C "$TAP_SEED" add -A
git -C "$TAP_SEED" commit -q -m "seed"
git -C "$TAP_SEED" push -q "$TAP" HEAD:main
export CLIPSYNC_TAP_URL="file://$TAP"
tap_commits() { git --git-dir="$TAP" rev-list --count main; }

make_stage
export STUB_CURL_SOURCE="$STAGE/assets/$DMG"
SEED_COUNT="$(tap_commits)"
: >"$LOGS/curl.log"

expect_refusal "tap refuses a draft release" "still a draft" env STUB_GH_VIEW_DRAFT=true bash "$TAP_SH"
expect_refusal "tap refuses a missing release" "no GitHub release v0.1.0" bash "$TAP_SH"
expect_refusal "tap refuses when the release tag is not on origin" "points at 'nothing'" \
  env STUB_GH_VIEW_DRAFT=false bash "$TAP_SH"
git -C "$REPO" push -q origin "$(git -C "$REPO" rev-parse HEAD~1):refs/tags/v0.1.0"
expect_refusal "tap refuses a tag that points at another commit" "not the staged commit" \
  env STUB_GH_VIEW_DRAFT=false bash "$TAP_SH"
git -C "$REPO" push -q --force origin "$(git -C "$REPO" rev-parse HEAD):refs/tags/v0.1.0"
if [[ ! -s "$LOGS/curl.log" ]]; then pass "tap downloads nothing before the release is public"; else fail "tap downloaded while unpublished"; fi

printf 'different bytes\n' >"$TMP/other.dmg"
expect_refusal "tap refuses a public DMG with a different checksum" "does not match the staged" \
  env STUB_GH_VIEW_DRAFT=false STUB_CURL_SOURCE="$TMP/other.dmg" bash "$TAP_SH"
expect_refusal "tap refuses when the public DMG cannot be downloaded" "could not download" \
  env STUB_GH_VIEW_DRAFT=false STUB_CURL_FAIL=1 bash "$TAP_SH"

cp "$STAGE/clipsync.rb" "$TMP/cask.backup"
sed -i.bak 's/sha256 "[0-9a-f]*"/sha256 "0000"/' "$STAGE/clipsync.rb"
expect_refusal "tap refuses a cask whose sha256 line was edited" "staged cask changed after staging" \
  env STUB_GH_VIEW_DRAFT=false bash "$TAP_SH"
cp "$TMP/cask.backup" "$STAGE/clipsync.rb"
printf '  postflight_steps do\n    run "/bin/sh", args: ["-c", "true"]\n  end\n' >>"$STAGE/clipsync.rb"
expect_refusal "tap refuses a cask with added steps" "staged cask changed after staging" \
  env STUB_GH_VIEW_DRAFT=false bash "$TAP_SH"
cp "$TMP/cask.backup" "$STAGE/clipsync.rb"
rm -f "$STAGE/clipsync.rb.bak"

if [[ "$(tap_commits)" == "$SEED_COUNT" ]]; then pass "refused tap runs leave the tap untouched"; else fail "a refused tap run changed the tap"; fi

: >"$LOGS/curl.log"
expect_success "tap pushes the cask once the release is public" env STUB_GH_VIEW_DRAFT=false bash "$TAP_SH"
if [[ "$(cat "$LOGS/curl.log")" == "https://github.com/example/clipsync/releases/download/v0.1.0/$DMG" ]]; then
  pass "tap checks the public release download URL"
else
  fail "tap downloaded from $(cat "$LOGS/curl.log")"
fi
if [[ "$(git --git-dir="$TAP" log -1 --format=%s main)" == "Update clipsync cask to v0.1.0" ]] \
  && git --git-dir="$TAP" show main:Casks/clipsync.rb | cmp -s - "$STAGE/clipsync.rb"; then
  pass "tap main has Casks/clipsync.rb identical to the staged cask"
else
  fail "tap commit or cask content is wrong"
fi
AFTER_COUNT="$(tap_commits)"
expect_success "tap rerun is a no-op" env STUB_GH_VIEW_DRAFT=false bash "$TAP_SH"
if [[ "$(tap_commits)" == "$AFTER_COUNT" && "$LAST_OUT" == *"nothing to push"* ]]; then
  pass "tap rerun adds no commit"
else
  fail "tap rerun changed the tap"
fi
git -C "$REPO" push -q origin :refs/tags/v0.1.0
unset CLIPSYNC_TAP_URL STUB_CURL_SOURCE CLIPSYNC_RELEASE_ROOT

# ===================== AUR package =====================
AUR_SH="$REPO/scripts/package/release-aur.sh"
AUR="$TMP/aur.git"
git init -q --bare "$AUR"
export CLIPSYNC_AUR_URL="file://$AUR" STUB_DOCKER_AUR=1
aur_commits() { git --git-dir="$AUR" rev-list --count master 2>/dev/null || echo 0; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
# pkgname, pkgver, arch, checksums and the x86_64 URL, read the way makepkg reads them.
pkgbuild_fields() {
  bash -c 'set -eu; source "$1"; printf "%s\n" "$pkgname" "$pkgver" "${arch[*]}" "${sha256sums[*]}" \
    "${sha256sums_x86_64[*]}" "${sha256sums_aarch64[*]:-none}" "${source_x86_64[*]}"' _ "$1"
}
last_box() { printf '%s/container-%s/work/pkg' "$LOGS" "$(cat "$LOGS/docker.count")"; }

make_stage
PUBLIC="$TMP/public"
mkdir -p "$PUBLIC"
cp "$STAGE/assets/$X86" "$STAGE/assets/$ARM" "$REPO/LICENSE-MIT" "$REPO/LICENSE-APACHE" "$PUBLIC/"
export STUB_CURL_DIR="$PUBLIC"
: >"$LOGS/curl.log"
: >"$LOGS/gh.log"

expect_success "aur --check builds the package before the release is public" bash "$AUR_SH" --check
if [[ ! -s "$LOGS/curl.log" && ! -s "$LOGS/gh.log" && "$(aur_commits)" == 0 ]]; then
  pass "aur --check contacts neither GitHub nor the AUR"
else
  fail "aur --check reached GitHub or the AUR"
fi
EXPECTED_FIELDS="clipsync-bin
0.1.0
x86_64 aarch64
$(sha "$REPO/LICENSE-MIT") $(sha "$REPO/LICENSE-APACHE")
$(sha "$STAGE/assets/$X86")
$(sha "$STAGE/assets/$ARM")
https://github.com/example/clipsync/releases/download/v0.1.0/$X86"
if [[ "$(pkgbuild_fields "$STAGE/aur/PKGBUILD")" == "$EXPECTED_FIELDS" ]]; then
  pass "PKGBUILD carries the staged version, arches and checksums"
else
  fail "PKGBUILD fields differ:"
  pkgbuild_fields "$STAGE/aur/PKGBUILD" | sed 's/^/    /' >&2
fi
if grep -qx '# Maintainer: Obedience Corp <contact@obediencecorp.com>' "$STAGE/aur/PKGBUILD" \
  && grep -qxF '	pkgver = 0.1.0' "$STAGE/aur/.SRCINFO"; then
  pass "aur --check writes PKGBUILD and .SRCINFO to the stage"
else
  fail "staged PKGBUILD or .SRCINFO is wrong"
fi
if cmp -s "$(last_box)/$X86" "$STAGE/assets/$X86" \
  && cmp -s "$(last_box)/clipsync-bin-0.1.0-LICENSE-MIT" "$REPO/LICENSE-MIT"; then
  pass "aur --check hands the container the staged archives and licenses"
else
  fail "the check container did not get the staged sources"
fi

make_stage passed false x86_64
expect_success "aur --check handles a release without aarch64" bash "$AUR_SH" --check
if [[ "$(pkgbuild_fields "$STAGE/aur/PKGBUILD" | sed -n '3p;6p')" == $'x86_64\nnone' ]] \
  && ! grep -q aarch64 "$STAGE/aur/PKGBUILD"; then
  pass "an x86_64-only release renders an x86_64-only PKGBUILD"
else
  fail "x86_64-only PKGBUILD still mentions aarch64"
fi

make_stage skipped
expect_refusal "aur refuses a stage that skipped the quality gates" "does not record passed quality gates" \
  env STUB_GH_VIEW_DRAFT=false bash "$AUR_SH"

make_stage
expect_refusal "aur refuses a malformed maintainer" "must look like 'Name <email>'" \
  env CLIPSYNC_AUR_MAINTAINER=nobody bash "$AUR_SH" --check
printf 'tampered\n' >>"$STAGE/assets/$X86"
expect_refusal "aur refuses an archive changed after staging" "changed after staging" bash "$AUR_SH" --check
make_stage
: >"$LOGS/curl.log"
expect_refusal "aur refuses a draft release" "still a draft" env STUB_GH_VIEW_DRAFT=true bash "$AUR_SH"
expect_refusal "aur refuses a missing release" "no GitHub release v0.1.0" bash "$AUR_SH"
expect_refusal "aur refuses when the release tag is not on origin" "points at 'nothing'" \
  env STUB_GH_VIEW_DRAFT=false bash "$AUR_SH"
git -C "$REPO" push -q origin "$(git -C "$REPO" rev-parse HEAD~1):refs/tags/v0.1.0"
expect_refusal "aur refuses a tag that points at another commit" "not the staged commit" \
  env STUB_GH_VIEW_DRAFT=false bash "$AUR_SH"
git -C "$REPO" push -q --force origin "$(git -C "$REPO" rev-parse HEAD):refs/tags/v0.1.0"
if [[ ! -s "$LOGS/curl.log" ]]; then pass "aur downloads nothing before the release is public"; else fail "aur downloaded while unpublished"; fi

OTHER_PUBLIC="$TMP/other-public"
mkdir -p "$OTHER_PUBLIC"
cp "$PUBLIC"/* "$OTHER_PUBLIC/"
printf 'different bytes\n' >"$OTHER_PUBLIC/$ARM"
expect_refusal "aur refuses a public archive with a different checksum" "does not match the staged release" \
  env STUB_GH_VIEW_DRAFT=false STUB_CURL_DIR="$OTHER_PUBLIC" bash "$AUR_SH"
cp "$PUBLIC"/* "$OTHER_PUBLIC/"
printf 'edited\n' >>"$OTHER_PUBLIC/LICENSE-MIT"
expect_refusal "aur refuses a public license that differs from the tagged commit" "does not match the staged release" \
  env STUB_GH_VIEW_DRAFT=false STUB_CURL_DIR="$OTHER_PUBLIC" bash "$AUR_SH"
expect_refusal "aur refuses when a public file cannot be downloaded" "could not download" \
  env STUB_GH_VIEW_DRAFT=false STUB_CURL_FAIL=1 bash "$AUR_SH"
expect_refusal "aur pushes nothing when the Arch Linux check fails" "failed its Arch Linux check" \
  env STUB_GH_VIEW_DRAFT=false STUB_DOCKER_START_FAIL=1 bash "$AUR_SH"
if [[ "$(aur_commits)" == 0 ]]; then pass "refused aur runs leave the AUR untouched"; else fail "a refused aur run pushed"; fi

: >"$LOGS/curl.log"
expect_success "aur pushes once the release is public" env STUB_GH_VIEW_DRAFT=false bash "$AUR_SH"
if grep -qxF "https://github.com/example/clipsync/releases/download/v0.1.0/$X86" "$LOGS/curl.log" \
  && grep -qxF "https://raw.githubusercontent.com/example/clipsync/v0.1.0/LICENSE-MIT" "$LOGS/curl.log"; then
  pass "aur checks the public archive and license URLs"
else
  fail "aur downloaded from unexpected URLs"
fi
if [[ ! -e "$(last_box)/$X86" ]]; then
  pass "a pushing run leaves the downloads to makepkg"
else
  fail "a pushing run handed makepkg local archives"
fi
if [[ "$(git --git-dir="$AUR" log -1 --format='%s|%an <%ae>|%cn <%ce>' master)" \
  == "Update to 0.1.0|Obedience Corp <contact@obediencecorp.com>|Obedience Corp <contact@obediencecorp.com>" ]]; then
  pass "AUR commit uses the maintainer identity, not the local git identity"
else
  fail "AUR commit identity: $(git --git-dir="$AUR" log -1 --format='%s|%an <%ae>|%cn <%ce>' master)"
fi
if git --git-dir="$AUR" show master:PKGBUILD | cmp -s - "$STAGE/aur/PKGBUILD" \
  && git --git-dir="$AUR" show master:.SRCINFO | cmp -s - "$STAGE/aur/.SRCINFO"; then
  pass "AUR master has the tested PKGBUILD and .SRCINFO"
else
  fail "AUR content differs from the tested files"
fi
AFTER_COUNT="$(aur_commits)"
expect_success "aur rerun is a no-op" env STUB_GH_VIEW_DRAFT=false bash "$AUR_SH"
if [[ "$(aur_commits)" == "$AFTER_COUNT" && "$LAST_OUT" == *"nothing to push"* ]]; then
  pass "aur rerun adds no commit"
else
  fail "aur rerun changed the AUR"
fi
expect_refusal "aur refuses to replace the same version with different content" "needs a pkgrel bump" \
  env STUB_GH_VIEW_DRAFT=false CLIPSYNC_AUR_MAINTAINER="Someone Else <else@example.invalid>" bash "$AUR_SH"
git -C "$REPO" push -q origin :refs/tags/v0.1.0
unset CLIPSYNC_AUR_URL STUB_DOCKER_AUR STUB_CURL_DIR CLIPSYNC_RELEASE_ROOT

# ===================== release-mode packaging guards =====================
FIX_BIN="$TMP/fixture-bin"
printf '#!/bin/sh\necho fixture\n' >"$FIX_BIN"
chmod 755 "$FIX_BIN"
expect_refusal "Linux release build refuses fixture binaries" "release builds cannot use fixture or prebuilt binaries" \
  env CLIPSYNC_PACKAGE_RELEASE=1 CLIPSYNC_PACKAGE_FIXTURE_MODE=1 CLIPSYNC_PACKAGE_SKIP_BUILD=1 \
  CLIPSYNC_PACKAGE_CLI_BIN="$FIX_BIN" CLIPSYNC_PACKAGE_GUI_BIN="$FIX_BIN" \
  CLIPSYNC_PACKAGE_DIST="$TMP/release-dist" bash "$SCRIPT_DIR/build-linux-companion.sh"
if [[ "$(uname -s)" == "Darwin" ]]; then
  expect_refusal "macOS release build refuses fixture binaries" "release builds cannot use fixture or prebuilt binaries" \
    env CLIPSYNC_PACKAGE_RELEASE=1 CLIPSYNC_PACKAGE_FIXTURE_MODE=1 CLIPSYNC_PACKAGE_SKIP_BUILD=1 \
    CLIPSYNC_PACKAGE_CLI_BIN="$FIX_BIN" CLIPSYNC_PACKAGE_GUI_BIN="$FIX_BIN" \
    CLIPSYNC_PACKAGE_DIST="$TMP/release-dist" bash "$SCRIPT_DIR/build-macos-companion.sh"
fi
if [[ ! -e "$TMP/release-dist" ]]; then pass "refused release builds write nothing"; else fail "refused release build wrote output"; fi

write_manifest() {
  cat >"$TMP/m.json" <<EOF
{
  "release_build": true,
  "signing": "$1",
  "notarized": $2,
  "public_release_ready": $2,
  "fixture_mode": $3,
  "features_verified": true,
  "archive": "$4"
}
EOF
}
write_manifest unsigned false false "$X86"
expect_success "verify accepts an unsigned release manifest" bash "$SCRIPT_DIR/verify.sh" --manifest "$TMP/m.json"
write_manifest unsigned false true "$X86"
expect_refusal "verify rejects a fixture-mode release manifest" "release builds cannot be fixture builds" \
  bash "$SCRIPT_DIR/verify.sh" --manifest "$TMP/m.json"
write_manifest unsigned true false "$X86"
expect_refusal "verify rejects an unsigned release claiming notarization" "cannot claim notarization" \
  bash "$SCRIPT_DIR/verify.sh" --manifest "$TMP/m.json"
write_manifest unsigned false false "clipsync-UNSIGNED-DEVELOPMENT.tar.gz"
expect_refusal "verify rejects a release manifest with a development label" "must not carry a DEVELOPMENT label" \
  bash "$SCRIPT_DIR/verify.sh" --manifest "$TMP/m.json"
write_manifest developer-id false false "$DMG"
expect_refusal "verify rejects a developer-id release that is not notarized" "must record notarized=true" \
  bash "$SCRIPT_DIR/verify.sh" --manifest "$TMP/m.json"

if [[ "$(uname -s)" == "Darwin" ]]; then
  # A release bundle must carry an intact signature seal.
  APP="$TMP/seal/ClipSync.app"
  mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
  cp /usr/bin/true "$APP/Contents/MacOS/clipsync-desktop"
  cp /usr/bin/true "$APP/Contents/MacOS/clipsync"
  cat >"$APP/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>clipsync-desktop</string>
<key>CFBundleIdentifier</key><string>com.clipsync.test</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF
  printf '{\n  "release_build": true,\n  "signing": "unsigned",\n  "fixture_mode": false\n}\n' \
    >"$APP/Contents/Resources/PROVENANCE.json"
  expect_refusal "verify rejects a release app without a bundle seal" "signature does not verify" \
    env PATH="$REAL_PATH" bash "$SCRIPT_DIR/verify.sh" --macos "$APP"
  /usr/bin/codesign --force --deep --sign - "$APP" 2>/dev/null
  expect_success "verify accepts an ad-hoc sealed release app" \
    env PATH="$REAL_PATH" bash "$SCRIPT_DIR/verify.sh" --macos "$APP"
  printf ' ' >>"$APP/Contents/Resources/PROVENANCE.json"
  expect_refusal "verify rejects a release app changed after signing" "signature does not verify" \
    env PATH="$REAL_PATH" bash "$SCRIPT_DIR/verify.sh" --macos "$APP"
fi

echo
echo "release tests: $PASS passed, $FAIL failed"
[[ "$FAIL" -eq 0 ]]
