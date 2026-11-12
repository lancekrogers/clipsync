# Changelog

All notable changes to ClipSync will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1] - 2026-11-12

### Fixed
- The Linux app menu entry lists only one main category (Utility), so menus no
  longer show ClipSync twice.

## [0.1.0] - 2026-11-12

First public release.

### Added
- Text and image clipboard sync between trusted macOS and Linux computers on
  the local network.
- A desktop app, a terminal UI (`clipsync tui`) and a command-line tool, all
  driving one background service.
- Pairing with a one-time six-digit code. Discovery alone never authorizes a
  device.
- Direct, mutually authenticated TLS 1.3 connections with Ed25519 device
  identities. There is no cloud service and no account.
- Clipboard history with search, image previews and copy-again. Deleting one
  copy or clearing all history also applies on connected devices.
- Pause and resume syncing.
- A macOS disk image for Apple Silicon (ad-hoc signed, not notarized), Linux
  archives for x86_64 and aarch64, a Homebrew cask and the `clipsync-bin` AUR
  package.

[Unreleased]: https://github.com/lancekrogers/clipsync/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/lancekrogers/clipsync/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/lancekrogers/clipsync/releases/tag/v0.1.0
