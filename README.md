# Windows Codex Stability Guardian

[![CI](https://github.com/ZRainbow1275/windows-codex-stability-guardian/actions/workflows/ci.yml/badge.svg)](https://github.com/ZRainbow1275/windows-codex-stability-guardian/actions/workflows/ci.yml)
[![Release](https://github.com/ZRainbow1275/windows-codex-stability-guardian/actions/workflows/release.yml/badge.svg)](https://github.com/ZRainbow1275/windows-codex-stability-guardian/actions/workflows/release.yml)

Windows-native local stability tooling for Codex CLI, Docker Desktop / WSL2, and Windows User Profile diagnostics.

Guardian is a conservative workstation operations tool built for a very specific job: detect recurring local failure classes, classify them from live machine evidence, and only repair them when the repair path is known, bounded, and explicitly confirmed.

## Why this project exists

Some workstation failures look like data loss, broken installs, or "just restart everything" incidents when they are actually caused by:

- local state drift
- stale SQLite rows
- Docker / WSL baseline drift
- user-profile locks or temporary-profile risk

Guardian is designed to make those problems observable first and repairable second.

Its operating principles are:

- **Observe before repair**
- **Backup before write**
- **Explicit confirmation before mutation**
- **Windows-native, low-overhead operation**
- **No dependence on remote services**

## What Guardian does

### Codex health and repair

Guardian can inspect a local Codex home, classify common `/resume` failure modes, detect project trust drift, and run a managed Codex repair flow when the known drift patterns are present.

Key behaviors:

- checks `history.jsonl`, rollout/session files, and the latest `state_*.sqlite`
- detects `threads.has_user_event` drift
- detects `/resume` slow-path drift from `codex-tui.log` when recent runs stall on `Loading sessions...`
- detects missing trusted project entries in `%USERPROFILE%\.codex\config.toml`
- runs the managed repair playbook only when you explicitly pass `--confirm`
- creates a SQLite backup before mutation
- creates a `config.toml` backup before appending trusted project entries
- creates a launcher backup before staging a controlled `vendor-hotfix` binary and patching the Codex launcher for validated `C4` slow-path cases
- verifies post-write state before declaring success
- writes a structured repair audit after execution

### Docker / WSL baseline recovery

Guardian inspects Docker Desktop, WSL state, and `.wslconfig`, then applies bounded recovery flows for known baseline or runtime anomalies.

Key behaviors:

- validates `.wslconfig` baseline keys
- distinguishes configuration drift from runtime recovery
- blocks high-risk runtime restart flows when running containers make that unsafe
- writes repair audits for confirm-mode actions

### Profile diagnostics

Guardian reads Windows profile-related event evidence and turns it into guided recovery steps without modifying the system.

Key behaviors:

- reads User Profile Service evidence
- classifies registry-lock and temporary-profile signals
- detects when security software involvement is likely
- exports structured diagnostic JSON

### Bundle export

Guardian can export a support bundle containing current health output, profile diagnostics, and audit summaries.

## When to use it

Guardian is a good fit when you want:

- a local-first Windows diagnostics tool
- a safer alternative to ad hoc workstation repair scripts
- structured health output before making system changes
- bounded repair paths for recurring Codex or Docker / WSL issues

Guardian is **not** positioned as:

- a cloud service
- a fleet-management platform
- a generic Windows optimizer
- an automatic registry fixer for user-profile corruption

## Command overview

| Command | Purpose | Mutation boundary |
| --- | --- | --- |
| `guardian.exe check` | Run workstation health checks | Read-only |
| `guardian.exe repair codex` | Inspect and optionally repair known Codex drift | Requires `--confirm` for writes |
| `guardian.exe repair docker` | Inspect and optionally repair Docker / WSL baseline/runtime issues | Requires `--confirm` for writes |
| `guardian.exe diagnose profile` | Export profile-related diagnostics and guided recovery notes | Read-only |
| `guardian.exe export bundle` | Export a support bundle from current health and audit data | Read-only with respect to monitored systems |
| `guardian.exe gui` | Launch the desktop GUI | No automatic repair without explicit action |
| `guardian.exe tray` | Launch the tray entry point | No automatic repair without explicit action |

## Quick start

### 1. Download a release

Get the latest release assets from:

- [Releases](https://github.com/ZRainbow1275/windows-codex-stability-guardian/releases)

Each release publishes:

- `guardian-v<version>-windows-x64.zip`
- `guardian.exe`
- `SHA256SUMS.txt`

### 2. Verify what you downloaded

Recommended verification flow:

1. Verify the Git tag is shown as signed / verified on GitHub.
2. Download the release zip and `SHA256SUMS.txt`.
3. Verify the checksum locally:

```powershell
Get-FileHash .\guardian-v0.1.0-windows-x64.zip -Algorithm SHA256
```

Compare the output against `SHA256SUMS.txt`.

### 3. Start with a read-only health check

```powershell
guardian.exe check --json
```

### 4. Escalate only when needed

```powershell
guardian.exe repair codex --dry-run --json
guardian.exe repair codex --confirm --json
guardian.exe repair docker --dry-run --json
guardian.exe diagnose profile --json --output profile-diagnosis.json
guardian.exe export bundle --json
guardian.exe tray
```

## Safety model

Guardian is intentionally conservative:

- `guardian.exe check` is always read-only
- write actions are gated behind `--confirm`
- Codex repairs back up the active SQLite state database first
- Codex slow-path launcher staging is only allowed inside `guardian repair codex --confirm` after Guardian has classified `C4` and found a verified local hotfix binary
- profile diagnostics never auto-edit `ProfileList` or terminate security software
- runtime restart flows for Docker / WSL are guarded by live machine state

## Operational data paths

By default Guardian uses:

- audits: `%LOCALAPPDATA%\guardian\audits`
- bundles: `%LOCALAPPDATA%\guardian\bundles`
- backups: `%LOCALAPPDATA%\guardian\backups`

## Repository layout

| Path | Purpose |
| --- | --- |
| `apps/guardian/` | Main application entry points, CLI, tray, GUI, and packaging script |
| `crates/guardian-core/` | Shared domain types, policies, and audit structures |
| `crates/guardian-observers/` | Read-only machine evidence collection and classifiers |
| `crates/guardian-repair/` | Repair orchestration, bundle export, and guarded write paths |
| `crates/guardian-windows/` | Windows-specific helpers for paths, event logs, process execution, and registry-facing utilities |
| `.github/workflows/` | CI and tag-driven release automation |

## Development

### Requirements

- Windows
- Rust stable toolchain
- PowerShell

### Local build

```powershell
cargo build -p guardian
cargo test --workspace
```

### Local release packaging

```powershell
.\apps\guardian\scripts\package-release.ps1 -Version v0.1.0
```

The packaging script writes artifacts under `dist\<version>\`.

## Project status

Current release line: **v0.1.0**

Implemented and wired:

- Codex health checks
- Codex managed repair orchestration for stale-row drift and trusted-project drift
- Docker / WSL checks and guarded repair flows
- profile diagnostics with guided recovery notes
- bundle export
- tray and GUI entry points

## Security and privacy

This public repository is intentionally kept free of machine-specific secrets, private session artifacts, and local workstation-only notes.

See:

- [SECURITY.md](./SECURITY.md)

## License

MIT
