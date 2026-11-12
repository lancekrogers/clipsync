# GitHub Workflows Disabled

All GitHub workflows have been temporarily disabled to prevent unnecessary CI/CD costs.

`release-packages.yml` is enabled. It runs only when a release is published (or
by hand) and updates the Homebrew tap and the AUR; see docs/RELEASE_CHECKLIST.md.

## Disabled Workflows:
- build.yml.disabled
- ci.yml.disabled  
- release.yml.disabled
- security.yml.disabled
- test.yml.disabled

## To Re-enable:
Remove the `.disabled` extension from any workflow file you want to re-enable:
```bash
cd .github/workflows/
mv build.yml.disabled build.yml
```

## To Re-enable All:
```bash
cd .github/workflows/
for file in *.yml.disabled; do mv "$file" "${file%.disabled}"; done
```