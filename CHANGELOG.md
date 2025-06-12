# Changelog

All notable changes to ClipSync will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial release of ClipSync
- Cross-platform clipboard synchronization (macOS and Linux)
- SSH key-based authentication
- WebSocket transport with automatic reconnection
- mDNS service discovery with manual fallback
- Encrypted clipboard history
- Command-line interface with subcommands
- Hotkey support for quick actions
- Automatic service management (systemd/launchd)
- Multi-device synchronization support
- Comprehensive test suite with >80% coverage
- Performance benchmarks and monitoring
- Security scanning in CI/CD pipeline

### Security
- All clipboard data encrypted at rest
- SSH key authentication for secure connections
- Regular security audits via GitHub Actions