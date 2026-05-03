# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog.

## [Unreleased]

## [0.1.2] - 2026-05-03

Guardian assumes the user has already installed `@openai/codex` via npm
(that is the prerequisite for needing this tool at all). The release zip
therefore ships only `guardian.exe`, the trusted repair script, and the
project docs — never a copy of `codex.exe`. The slow-path (C4) launcher
patch falls back to the user's own `vendor/<triple>/codex/codex.exe` from
their npm install when no `vendor-hotfix/...` is present.

### Fixed

- `guardian repair codex --confirm` no longer aborts with
  `trusted repair script is missing: ...\.codex\tools\repair-codex-resume.ps1`
  on a freshly-installed machine (GitHub issue #2). The trusted PowerShell
  repair script is now embedded into `guardian.exe` via `include_str!` and
  materialized to `<codex_home>/tools/repair-codex-resume.ps1` on first launch
  (and defensively before the C2 repair branch). Existing operator-customised
  scripts at that path are preserved.

### Changed

- `package-release.ps1` now stages `tools/repair-codex-resume.ps1` inside the
  release zip as a defense-in-depth copy of the trusted script.

### Added

- `guardian-repair::codex::ensure_codex_tools_deployed` helper, called from
  `apps/guardian/src/app.rs::run` so every CLI / GUI / tray entry point lays
  the embedded repair script down idempotently.
- `apps/guardian/assets/tools/repair-codex-resume.ps1` is now the canonical
  source of truth for the repair script and is shipped with the source tree.

## [0.1.1] - 2026-04-23

### Fixed

- `guardian repair codex --confirm` no longer aborts the entire run and discards
  successful stale-row (`C2`) or trusted-project (`C6`) repair work when the
  slow-path (`C4`) launcher hotfix step fails (for example, when no verified
  hotfix binary is present on the workstation, or when the launcher vendor
  block can no longer be located). The failure is captured as
  `repair_slow_path_error` evidence, recorded in the audit record, surfaced in
  CLI / GUI / tray notes, and the outcome is reported as `unresolved` instead of
  a hard error. This preserves the audit trail and any earlier successful
  repair work on machines where the hotfix source is not yet staged.

### Documentation

- README now states the project's creation purpose and the concrete failure
  classes it was built to handle.

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
