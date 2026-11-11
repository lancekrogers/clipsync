# Cutting a ClipSync release

A release publishes the version in `Cargo.toml` as GitHub release `v<version>`
with a macOS disk image, Linux archives, `SHA256SUMS` and release notes, then
updates the Homebrew cask. Three commands do it; making the release public is
a manual click in between.

| Step | Command | Writes to GitHub |
|---|---|---|
| Stage | `just companion-release` | Nothing (reads releases and tags) |
| Draft | `just companion-release-publish` | A **draft** release with the staged files |
| Publish | `gh release edit v<version> --repo lancekrogers/clipsync --draft=false`, or **Publish release** on the draft | The tag and the public release |
| Tap | `just companion-release-tap` | `Casks/clipsync.rb` in `lancekrogers/homebrew-tap` |

## Before staging

1. Bump `version` in both `Cargo.toml` and `crates/desktop/Cargo.toml` to the
   same value, update `CHANGELOG.md`, and merge to `main`.
2. Check out `main` at `origin/main` with a clean tree (no untracked files).
3. Have Docker running on an Apple Silicon Mac. The Linux build image needs a
   few GB of free space in the Docker VM.
4. Optional signing: with `CLIPSYNC_CODESIGN_IDENTITY` (a Developer ID
   Application identity) and notary credentials (`CLIPSYNC_NOTARY_PROFILE`, or
   `CLIPSYNC_NOTARY_APPLE_ID` + `CLIPSYNC_NOTARY_TEAM_ID` +
   `CLIPSYNC_NOTARY_PASSWORD`) set, staging signs and notarizes through
   `scripts/package/sign-macos.sh`. Setting only some of them is refused.
   Without any, the app gets an ad-hoc signature and every name and note says
   it is unsigned.

## Stage: `just companion-release`

Refuses, without building anything, when:

- the tree has uncommitted or untracked files, HEAD is not `main`, or HEAD does
  not equal `origin/main` after a fetch;
- the two crate versions disagree;
- tag `v<version>` exists locally or on origin, or a GitHub release or draft
  for it exists (or the release list cannot be read);
- fixture or prebuilt binaries are configured (`CLIPSYNC_PACKAGE_FIXTURE_MODE`,
  `CLIPSYNC_PACKAGE_SKIP_BUILD`, `CLIPSYNC_PACKAGE_CLI_BIN`,
  `CLIPSYNC_PACKAGE_GUI_BIN`);
- `dist/release/v<version>` already exists (move or delete it to restage);
- `cargo +1.95.0 fmt --all -- --check`, `clippy --workspace --all-features --
  -D warnings` or `test --workspace --all-features` fails.

Then it builds into `dist/release/v<version>/`:

- **macOS:** `ClipSync.app` from this checkout, sealed with an ad-hoc signature
  (or Developer ID signed and notarized), in
  `ClipSync-<version>-macos-arm64-unsigned.dmg`. The app is checked with
  `verify.sh` and `codesign --verify --deep --strict`, both in place and from
  the mounted disk image.
- **Linux:** `clipsync-<version>-linux-x86_64.tar.gz` and `-aarch64.tar.gz`,
  built in Docker (`scripts/package/Dockerfile.release-linux`, Ubuntu 22.04)
  from a `git archive` export of HEAD. x86_64 is cross-compiled. Each archive
  is unpacked and checked with `verify.sh`, and its required glibc version is
  read from the binaries.
- `assets/SHA256SUMS`, `RELEASE_NOTES.md` (from
  `scripts/package/release-notes.md.in`), `clipsync.rb` (the Homebrew cask) and
  `release.json`, written last; without it the stage counts as incomplete.

`--skip-gates` (`just companion-release --skip-gates`) is for iterating on
packaging. It is recorded in `release.json` and publish refuses the result.

Read `RELEASE_NOTES.md` before publishing.

## Draft: `just companion-release-publish`

Refuses unless `release.json` exists, every staged file still matches its
checksum in `release.json` and `SHA256SUMS`, no unlisted files are in
`assets/`, gates ran, HEAD is the staged commit and that commit is on
`origin/main`, and the tag and release still do not exist. It then runs
`gh release create v<version> --draft --target <commit>` with the notes and
assets and prints the draft URL. A draft creates no tag.

## Publish

Open the draft, check the notes and files, then click **Publish release** or
run the `gh release edit … --draft=false` command it printed. This creates the
tag at the staged commit.

## Tap: `just companion-release-tap`

Refuses unless the release is published (not a draft) and the disk image
downloaded from its public URL has the staged checksum. It then commits
`Casks/clipsync.rb` to the tap's `main` branch and pushes, the way the tap is
maintained. Afterwards `brew install --cask lancekrogers/tap/clipsync` works.
The tap also holds an older, unbuildable `Formula/clipsync.rb`; always pass
`--cask`.

## Testing the commands

`just companion-package-test` runs the release-command tests in
`scripts/package/test_release.sh` against a throwaway repository with stub
`gh`, `cargo`, `docker` and `curl`. They cover every refusal above and check
the exact `gh release create` arguments; nothing reaches GitHub.
