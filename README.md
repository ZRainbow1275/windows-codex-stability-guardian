# Windows Codex Stability Guardian

Low-overhead Windows-native stability tooling for Codex CLI, Docker Desktop / WSL2, and Windows User Profile incidents.

## Overview

Windows Codex Stability Guardian is a local-first operations tool designed for a single Windows workstation. It focuses on recurring failure classes that are easy to misdiagnose as "data loss" but are often caused by drift in local state, runtime health, or user-profile locks.

The project ships a single Rust-based executable, `guardian.exe`, with CLI, tray, and GUI entry points. Its core design goals are:

- **Observe before repair**
- **Backup before write**
- **Explicit confirmation before mutation**
- **Windows-native, low-overhead operation**
- **No dependence on remote services**

## What the tool does

### 1. Codex health and repair

Guardian can inspect a local Codex home, classify common `/resume` failure modes, and execute a controlled stale-row repair flow when the failure matches the known drift pattern.

Key behaviors:

- checks `history.jsonl`, rollout/session files, and the latest `state_*.sqlite`
- detects `threads.has_user_event` drift
- runs the trusted repair playbook only when you explicitly pass `--confirm`
- creates a SQLite backup before mutation
- writes a structured repair audit after execution

### 2. Docker / WSL baseline recovery

Guardian inspects Docker Desktop, WSL state, and `.wslconfig`, then applies bounded recovery flows for known baseline or runtime anomalies.

Key behaviors:

- validates `.wslconfig` baseline keys
- distinguishes configuration drift from runtime recovery
- blocks high-risk runtime restart flows when running containers make that unsafe
- writes repair audits for confirm-mode actions

### 3. Profile diagnostics

Guardian reads Windows profile-related event evidence and turns it into guided recovery steps without modifying the system.

Key behaviors:

- reads User Profile Service evidence
- classifies registry-lock and temporary-profile signals
- detects when security software involvement is likely
- exports structured diagnostic JSON

### 4. Bundle export

Guardian can export a support bundle containing current health output, profile diagnostics, and audit summaries.

## Command surface

```text
guardian.exe check
guardian.exe repair codex
guardian.exe repair docker
guardian.exe diagnose profile
guardian.exe export bundle
guardian.exe gui
guardian.exe tray
```

### Typical examples

```powershell
guardian.exe check --json
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
- profile diagnostics never auto-edit `ProfileList` or terminate security software
- runtime restart flows for Docker / WSL are guarded by live machine state

## Release artifacts

Release builds are packaged as Windows x64 zip archives containing:

- `guardian.exe`
- `README.md`
- `CHANGELOG.md`
- `LICENSE`

Each release also generates:

- `SHA256SUMS.txt`

## Signing and verification

This repository is prepared for:

- **SSH-signed commits**
- **SSH-signed tags**
- **SHA256 release checksums**

Recommended verification flow:

1. Verify the GitHub tag is shown as signed / verified.
2. Download the release zip and `SHA256SUMS.txt`.
3. Verify the checksum locally:

```powershell
Get-FileHash .\guardian-v0.1.0-windows-x64.zip -Algorithm SHA256
```

Compare the output against `SHA256SUMS.txt`.

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

## Operational data paths

By default Guardian uses:

- audits: `%LOCALAPPDATA%\guardian\audits`
- bundles: `%LOCALAPPDATA%\guardian\bundles`
- backups: `%LOCALAPPDATA%\guardian\backups`

## Project status

Current release line: **v0.1.0**

Implemented and wired:

- Codex health checks
- Codex stale-row repair orchestration
- Docker / WSL checks and guarded repair flows
- profile diagnostics with guided recovery notes
- bundle export
- tray and GUI entry points

## License

MIT
