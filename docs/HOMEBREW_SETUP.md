# Homebrew Distribution Setup

## Quick Setup (Personal Tap) - 1-2 hours

### 1. Create GitHub Release
```bash
# Tag and create release
git tag -a v0.1.0 -m "Release v0.1.0"
git push origin v0.1.0

# Build binaries
make build-all
make package

# Upload to GitHub releases manually or use gh CLI:
gh release create v0.1.0 dist/*.tar.gz --title "v0.1.0" --notes "First release"
```

### 2. Create Homebrew Tap Repository
1. Create new repo: `homebrew-tap` on GitHub
2. Add formula: `Formula/clipsync.rb`
3. Update SHA256 hashes in formula after release

### 3. Users Install With:
```bash
brew tap lancekrogers/tap
brew install clipsync
```

## Alternative: GitHub Actions Automation - 3-4 hours
Add `.github/workflows/release.yml` to automate the entire process.

## Getting into Homebrew Core - Several weeks/months
- Need 30+ GitHub stars
- Pass strict quality requirements
- Maintain for 6+ months
- Submit PR to homebrew-core

## For Now: Update README
Change the installation to:
```bash
# Coming soon - for now use:
brew tap lancekrogers/tap
brew install clipsync
```