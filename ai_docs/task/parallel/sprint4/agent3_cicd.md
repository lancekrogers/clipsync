# Agent 3: CI/CD & Quality Assurance Specialist

## Your Mission
You are responsible for setting up a robust CI/CD pipeline that ensures ClipSync is built, tested, and released reliably across all platforms. You'll also perform final quality assurance and security audits.

## Context
- ClipSync is a Rust application targeting macOS and Linux
- GitHub Actions is the CI/CD platform
- Releases should include pre-built binaries for all platforms
- Security and performance are critical

## Your Tasks

### 1. GitHub Actions Workflows

#### .github/workflows/build.yml
- Trigger on: push, pull request
- Build matrix:
  - OS: ubuntu-latest, macos-latest
  - Rust: stable, beta, nightly (allow nightly failures)
  - Architecture: x86_64, aarch64
- Steps:
  - Cache Cargo dependencies
  - Run `cargo fmt -- --check`
  - Run `cargo clippy -- -D warnings`
  - Run `cargo build --release`
  - Upload artifacts

#### .github/workflows/test.yml
- Comprehensive test suite:
  - Unit tests: `cargo test`
  - Integration tests: `cargo test --test '*'`
  - Doc tests: `cargo test --doc`
- Code coverage with tarpaulin or similar
- Test on different OS versions
- Memory leak detection with valgrind (Linux)
- Performance regression tests

#### .github/workflows/release.yml
- Trigger on: version tags (v*)
- Build release binaries for all platforms
- Cross-compilation for ARM
- Strip binaries and optimize size
- Create GitHub release
- Upload binaries as release assets
- Generate changelog from commits
- Update Homebrew formula
- Publish to crates.io (optional)

#### .github/workflows/security.yml
- Run on schedule (weekly) and PRs
- Security audits:
  - `cargo audit` for dependencies
  - SAST scanning
  - License compliance check
- Dependency updates with Dependabot

### 2. Build Infrastructure

#### Cross-Compilation Setup
- Set up cross-compilation toolchains
- Build for:
  - macOS x86_64 and aarch64
  - Linux x86_64 and aarch64
- Use `cross` or Docker for Linux builds
- Document build requirements

#### Release Scripts
Create `scripts/release.sh`:
- Bump version in Cargo.toml
- Update CHANGELOG.md
- Create git tag
- Trigger release workflow
- Post-release cleanup

### 3. Quality Assurance

#### Test Matrix
Create comprehensive test scenarios:
- Fresh install on each platform
- Upgrade from previous version
- Multiple device sync
- Large clipboard content (>10MB)
- Network interruption handling
- Service restart resilience
- Permission edge cases

#### Performance Benchmarks
- Startup time benchmarks
- Memory usage profiling
- Sync speed testing
- CPU usage monitoring
- Create performance regression tests
- Set acceptable thresholds

#### Security Audit
- Review all dependencies
- Check for unsafe code usage
- Verify encryption implementation
- Network security review
- File permission checks
- Input validation audit

### 4. Release Automation

#### Version Management
- Semantic versioning
- Automatic version bumping
- Git tag creation
- Branch protection rules

#### Changelog Generation
- Auto-generate from commit messages
- Group by feature/fix/docs
- Include contributor credits
- Migration notes for breaking changes

#### Distribution
- Upload to GitHub releases
- Update package repositories
- Notify package maintainers
- Update website/docs

### 5. Monitoring & Metrics

#### Build Metrics
- Build success rate
- Test coverage trends
- Build time optimization
- Artifact sizes

#### Release Health
- Download statistics
- Issue tracking
- Crash reporting setup
- User feedback collection

## Important Considerations
- **Reproducible Builds**: Ensure builds are deterministic
- **Signing**: Set up code signing for releases
- **Caching**: Optimize CI time with smart caching
- **Secrets**: Properly manage API keys and certs
- **Notifications**: Set up failure notifications
- **Documentation**: Document the CI/CD process

## Files to Create/Modify
```
.github/
  workflows/
    build.yml
    test.yml
    release.yml
    security.yml
  dependabot.yml
scripts/
  release.sh
  cross-compile.sh
  benchmark.sh
tests/
  integration_test.rs
  performance_test.rs
Makefile or justfile (updated)
.cargo/config.toml (for cross-compilation)
```

## Deliverables
1. Complete CI/CD pipeline
2. Automated release process
3. Security scanning integration
4. Performance benchmarks
5. Quality assurance report

## Success Metrics
- All commits build successfully
- Test coverage > 80%
- Release process < 30 minutes
- Zero security vulnerabilities
- Performance within 10% of baseline

## Testing Your Work
- Trigger builds on multiple scenarios
- Test the full release process
- Verify artifacts work on target platforms
- Ensure rollback procedures work
- Document any manual steps

Remember: A good CI/CD pipeline makes releases boring and predictable!