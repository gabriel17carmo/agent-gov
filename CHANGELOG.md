# Changelog

All notable changes follow [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and semantic
versioning.

## [Unreleased]

## [0.1.1] - 2026-08-02

### Added

- Automated version-driven releases with recovery-safe republishing.
- Scheduled fuzzing for shell parsing, rewriting, and host-hook payloads.
- Private per-job control sockets for cancellation without signaling metadata PIDs.
- Build-provenance attestations for release artifacts.

### Changed

- Installation examples download the versioned installer from GitHub Releases.
- GitHub Actions are pinned to immutable commits and workflow syntax is checked in CI.

## [0.1.0] - 2026-08-02

### Added

- Initial public-preview implementation.
- Capacity 1/2 kernel-lock scheduler and bounded lease queue.
- Process-group supervisor with inherited I/O and execution timeout.
- Conservative tree-sitter Bash classification and insertion-only rewrite.
- Claude Code, Cursor, and RTK composition.
- Transactional hook installation, status, doctor, drain/resume, cancel, and safe capacity changes.
- Unit, contract, property, and real multi-process concurrency tests.

[Unreleased]: https://github.com/gabriel17carmo/agent-gov/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/gabriel17carmo/agent-gov/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/gabriel17carmo/agent-gov/releases/tag/v0.1.0
