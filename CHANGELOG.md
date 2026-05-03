# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog.

## [Unreleased]

### Changed

- `release` GitHub Actions workflow now clones `openai/codex` at a pinned ref
  (`CODEX_DEFAULT_REF`, default `rust-vrust-v0.122.0-alpha.4`) and builds
  `codex-cli` from source on the runner. The resulting `codex.exe` is mirrored
  to the `%TEMP%\codex-src\codex-rs\target\release\` candidate path so
  `package-release.ps1` finds it without a maintainer-staged
  `vendor-hotfix/` cache. v0.1.2 and earlier release zips silently shipped
  without the slow-path C4 hotfix because the workflow only had
  `package-release.ps1` and no Codex source available; subsequent tag pushes
  will now produce zips that include `vendor-hotfix/<triple>/codex/codex.exe`.
- The release workflow gained a `workflow_dispatch.inputs.codex_ref` override
  so maintainers can manually re-package an existing tag against a different
  Codex commit without editing the workflow.
- `package-release.ps1` is now invoked with `-HotfixSha256 ""` from CI, since
  the SHA256 of a freshly-built `codex.exe` cannot match the hard-coded
  default `927ece82…` (build environment differs from any prior maintainer
  copy). The default SHA check still applies for local maintainer runs.

## [0.1.2] - 2026-05-03

### Fixed

- `guardian repair codex --confirm` no longer aborts with
  `trusted repair script is missing: ...\.codex\tools\repair-codex-resume.ps1`
  on a freshly-installed machine (GitHub issue #2). The trusted PowerShell
  repair script is now embedded into `guardian.exe` via `include_str!` and
  materialized to `<codex_home>/tools/repair-codex-resume.ps1` on first launch
  (and defensively before the C2 repair branch). Existing operator-customised
  scripts at that path are preserved.
- The release zip again carries the slow-path Codex hotfix binary at
  `vendor-hotfix/x86_64-pc-windows-msvc/codex/codex.exe` so C4 (slow-path
  launcher) repair can run on machines that have never built Codex from
  source. v0.1.1 silently dropped this asset whenever the maintainer's
  `%TEMP%\codex-src\codex-rs\target\release\codex.exe` was missing.

### Changed

- `package-release.ps1` learned a multi-source hotfix discovery chain
  (repo `vendor-hotfix/` cache → prior `dist/v*/` releases → npm-installed
  `@openai/codex` package → `%TEMP%\codex-src` source build) plus a
  `-HotfixSha256` integrity check (default
  `927ece82f53d23383fc70b21d3b3c35fc024e0bfae76bc548f98f9295cad2c89`) and a
  new `-HotfixBinary` override for explicit paths. When no candidate matches
  the expected SHA256, the script now emits the full search list rather than
  a generic warning.
- The release zip also stages `tools/repair-codex-resume.ps1` as a
  defense-in-depth copy so the script is observable even before Guardian has
  ever run.

### Added

- `guardian-repair::codex::ensure_codex_tools_deployed` helper, called from
  `apps/guardian/src/app.rs::run` so every CLI / GUI / tray entry point lays
  down the bundled tools idempotently.
- `apps/guardian/assets/tools/repair-codex-resume.ps1` is now the canonical
  source of truth for the repair script and is shipped with the source tree.
- `vendor-hotfix/` repo-root cache convention (gitignored) so maintainers can
  stake a verified Codex hotfix `codex.exe` once and have every subsequent
  `package-release.ps1` invocation pick it up deterministically.

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
