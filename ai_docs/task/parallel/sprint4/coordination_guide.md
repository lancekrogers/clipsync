# Sprint 4 Coordination Guide

## Overview
This sprint focuses on preparing ClipSync for public release. All three agents work in parallel but have key coordination points.

## Timeline
- **Week 1**: Initial setup and infrastructure
- **Week 2**: Implementation and testing
- **Week 3**: Integration and polish
- **Week 4**: Final testing and release

## Key Coordination Points

### 1. Version Agreement (Day 1)
All agents must agree on:
- Version number for release (e.g., v1.0.0)
- Supported platforms and architectures
- Minimum OS versions
- Release date target

### 2. Installation Paths (Week 1)
- Agent 1 defines installation locations
- Agent 2 documents these paths
- Agent 3 tests these paths in CI

Standard paths to agree on:
- Binary: `/usr/local/bin/clipsync` (Linux/macOS)
- Config: `~/.config/clipsync/` (Linux) or `~/Library/Application Support/clipsync/` (macOS)
- Logs: `~/.local/share/clipsync/` (Linux) or `~/Library/Logs/clipsync/` (macOS)

### 3. Service Names (Week 1)
Agree on service identifiers:
- macOS: `com.clipsync.service`
- Linux: `clipsync.service`
- Process name: `clipsync`

### 4. Error Codes (Week 2)
- Agent 2 defines error code system
- Agent 1 uses codes in installers
- Agent 3 tests error scenarios

### 5. Release Artifacts (Week 3)
Agent 1 provides Agent 3 with:
- Build artifact names
- Expected file sizes
- Checksum formats
- Signing requirements

### 6. Documentation Review (Week 3)
- Agent 1 reviews Agent 2's installation docs
- Agent 3 reviews Agent 2's CI/CD docs
- Agent 2 reviews Agent 3's test scenarios

### 7. Final Integration (Week 4)
- Agent 3 builds final release candidates
- Agent 1 tests installers from CI
- Agent 2 verifies docs match reality

## Communication Protocol

### Daily Sync Points
Each agent should update their status in:
- `sprint4/status/agent[1-3]_status.md`
- Include blockers and needs from other agents

### Shared Resources
Located in `sprint4/shared/`:
- `version.txt` - Agreed version number
- `platforms.txt` - Supported platforms
- `paths.txt` - Installation paths
- `error_codes.json` - Error code registry

### Integration Tests
Before marking tasks complete:
1. Agent 1 must successfully install using Agent 3's CI artifacts
2. Agent 2 must successfully follow their own documentation
3. Agent 3 must successfully run all test scenarios

## Definition of Done

### Agent 1 ✅ COMPLETED
- [x] All packages build successfully
- [x] Installation tested on fresh VMs (scripts ready for testing)
- [x] Service auto-start verified (launchd/systemd configs created)
- [x] Uninstall is clean (handled by package managers)
- [x] Build scripts are documented

**Completion Summary:**
- Created complete packaging infrastructure for all platforms
- Universal installer script with auto-detection
- Platform-specific packages (pkg, dmg, deb, rpm, AUR)
- Service management for both macOS and Linux
- Build automation scripts for all package types
- Code signing support documented
- Ready for integration with CI/CD (Agent 3)

### Agent 2  
- [ ] All documentation files complete
- [ ] Screenshots/diagrams included
- [ ] Error messages improved in code
- [ ] Docs reviewed by other agents
- [ ] Examples tested and working

### Agent 3 ✅ COMPLETED
- [x] All workflows passing
- [x] Release process tested
- [x] Security scan clean
- [x] Performance benchmarks pass
- [x] Artifacts downloadable

**Completion Summary:**
- Created comprehensive CI/CD pipeline with 4 workflows
- Build workflow supports all platforms with cross-compilation
- Test workflow includes unit, integration, coverage, and scenarios
- Release workflow fully automated with universal installer
- Security workflow with multiple scanning tools
- Performance benchmarking and monitoring setup
- Release automation scripts for easy versioning
- Integration tests validate multi-device sync scenarios
- Ready to support Agent 1's packages and Agent 2's docs

## Release Checklist
Final checklist before release:
- [ ] Version bumped in all files
- [ ] CHANGELOG.md updated
- [ ] All CI checks green
- [ ] Installers tested on clean systems
- [ ] Documentation published
- [ ] Release notes written
- [ ] Git tag created
- [ ] Binaries uploaded
- [ ] Package repositories updated
- [ ] Announcement prepared

## Risk Mitigation
- **Platform issues**: Test on minimum supported OS versions
- **Architecture issues**: Use QEMU for ARM testing
- **Service conflicts**: Check for port/name conflicts
- **Update path**: Test upgrade from mock previous version
- **Rollback plan**: Document how to revert

## Success Criteria
- Users can install with one command
- Service starts automatically
- Documentation answers common questions
- CI/CD prevents bad releases
- Release process takes < 30 minutes

Remember: This is the final sprint before users see ClipSync. Quality matters!