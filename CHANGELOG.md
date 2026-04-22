# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog.

## [Unreleased]

## [0.1.0] - 2026-04-19

### Added

- Rust workspace for the Windows Codex Stability Guardian main program.
- `guardian.exe` command surface with:
  - `check`
  - `repair codex`
  - `repair docker`
  - `diagnose profile`
  - `export bundle`
  - `gui`
  - `tray`
- Codex managed repair orchestration with:
  - pre-flight health inspection
  - backup-first SQLite repair
  - project trust drift detection for `.codex/config.toml`
  - confirm-gated trusted project entry append with post-write verification
  - structured confirm-mode audit output
- Docker / WSL repair flows with guarded runtime restart behavior.
- Profile diagnosis output with guided recovery notes.
- Bundle export support for health, diagnosis, and audit summaries.
- Release packaging script for Windows x64 artifacts.
- GitHub Actions CI and tag-driven release workflow.

### Security

- Explicit `--confirm` gating for mutating repair actions.
- Read-only profile diagnosis boundary.
- Backup creation before Codex SQLite repair.

### Documentation

- Initial professional repository README.
- Initial release-ready changelog.
- Release packaging and verification guidance.
