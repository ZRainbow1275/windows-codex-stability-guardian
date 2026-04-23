# Windows Codex Stability Guardian

## 社区支持
学 AI , 上 L 站

[LinuxDO](https://linux.do/)

[![CI](https://github.com/ZRainbow1275/windows-codex-stability-guardian/actions/workflows/ci.yml/badge.svg)](https://github.com/ZRainbow1275/windows-codex-stability-guardian/actions/workflows/ci.yml)
[![Release](https://github.com/ZRainbow1275/windows-codex-stability-guardian/actions/workflows/release.yml/badge.svg)](https://github.com/ZRainbow1275/windows-codex-stability-guardian/actions/workflows/release.yml)

Windows-native local stability tooling for Codex CLI, Docker Desktop / WSL2, and Windows User Profile diagnostics.

Guardian is a conservative workstation operations tool built for a very specific job: detect recurring local failure classes, classify them from live machine evidence, and only repair them when the repair path is known, bounded, and explicitly confirmed.

## Why this project exists

Guardian was created to solve a concrete problem: developers on Windows workstations lose hours every week to a small set of recurring local failures that look scarier than they are, and the usual response — reinstall, reboot, re-clone — either hides the underlying drift or destroys work that could have been saved.

Typical symptoms Guardian was built for:

- Codex CLI `/resume` stops loading recent sessions or stalls on `Loading sessions…`
- Codex rejects an otherwise legitimate project path because its trusted-project list has drifted
- Docker Desktop or WSL2 refuses to start cleanly after a Windows update, or `.wslconfig` no longer matches the baseline the team agreed on
- Windows logs the user into a temporary profile, or `ProfileList` shows registry-lock evidence that a naive fix could make worse

What these incidents have in common:

- The root cause is local state drift, not code
- The safe fix is narrow and well-understood, but risky to run by hand
- Running the wrong "fix" (an ad-hoc registry edit, a full Codex reinstall, a force-kill of `vmmem`) can turn a 30-second drift into a day of recovery

Guardian's job is to make that class of problem **observable first** and **repairable second**, and to refuse to touch anything it cannot classify. It is the tool a careful operator would build for themselves after being burned once too often by "just reinstall it" advice.

Operating principles:

- **Observe before repair** — every repair starts from live machine evidence, not from assumptions
- **Backup before write** — SQLite state, `config.toml`, and Codex launcher are backed up before any mutation
- **Explicit confirmation before mutation** — `--confirm` is required; no background auto-fix
- **Bounded repair surface** — Guardian only repairs failure classes it can classify from evidence; anything else is reported, not touched
- **Windows-native, low-overhead operation** — runs on the workstation, uses Windows Event Log, registry reads, and PowerShell where appropriate
- **No dependence on remote services** — no telemetry, no cloud, no account required

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

- Codex health checks (history, sessions, state SQLite, TUI log, trusted-project config)
- Codex managed repair orchestration for stale-row drift (`C2`), trusted-project drift (`C6`), and `/resume` slow-path drift (`C4`) with guarded launcher hotfix staging
- Codex repair is fail-soft: a missing or unverifiable hotfix binary no longer discards successful stale-row or trust repair work; the skip reason is captured in the audit record and surfaced to CLI, GUI, and tray output
- Docker / WSL checks and guarded `.wslconfig` / runtime repair flows
- profile diagnostics with guided recovery notes
- bundle export with retention
- tray and GUI entry points that invoke the same repair pipeline as the CLI

## Security and privacy

This public repository is intentionally kept free of machine-specific secrets, private session artifacts, and local workstation-only notes.

See:

- [SECURITY.md](./SECURITY.md)

## License

MIT
