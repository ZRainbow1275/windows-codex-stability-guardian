[CmdletBinding()]
param(
    [string]$TargetVersion = "",
    [string]$CodexHome = "",
    [string]$StateDbPath = "",
    [string]$RepairCwd = "",
    [switch]$SkipInstall,
    [switch]$ForceInstall
)

# GuardianCodexResumeRepair/2026-06-30-scoped-resume-v8

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($CodexHome)) {
    $CodexHome = Join-Path $env:USERPROFILE ".codex"
}

function Write-Step {
    param(
        [string]$Message
    )

    Write-Host "[codex-resume-repair] $Message"
}

function Write-WarnStep {
    param(
        [string]$Message
    )

    Write-Warning "[codex-resume-repair] $Message"
}

function Get-CodexCommandInfo {
    try {
        return Get-Command codex -ErrorAction Stop | Select-Object -First 1
    } catch {
        return $null
    }
}

function Get-CodexVersionText {
    try {
        return (& codex --version 2>$null | Select-Object -First 1)
    } catch {
        return $null
    }
}

function Get-CodexVersion {
    param(
        [string]$VersionText
    )

    if ([string]::IsNullOrWhiteSpace($VersionText)) {
        return $null
    }

    $match = [regex]::Match($VersionText, "codex-cli\s+(\d+\.\d+\.\d+)")
    if (-not $match.Success) {
        return $null
    }

    return [version]$match.Groups[1].Value
}

function Get-TextOrFallback {
    param(
        [string]$Value,
        [string]$Fallback
    )

    if ([string]::IsNullOrWhiteSpace($Value)) {
        return $Fallback
    }

    return $Value
}

function Get-FileLineCount {
    param(
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return 0
    }

    return (Get-Content -LiteralPath $Path | Measure-Object -Line).Lines
}

function Get-RolloutFileCount {
    param(
        [string]$SessionsRoot
    )

    if (-not (Test-Path -LiteralPath $SessionsRoot)) {
        return 0
    }

    return @(
        Get-ChildItem -LiteralPath $SessionsRoot -Filter "rollout-*.jsonl" -Recurse -File -ErrorAction SilentlyContinue
    ).Count
}

function Get-LatestStateDbPath {
    param(
        [string]$CodexHome
    )

    if (-not (Test-Path -LiteralPath $CodexHome)) {
        return $null
    }

    $candidates = @(
        Get-ChildItem -LiteralPath $CodexHome -Filter "state_*.sqlite" -File -ErrorAction SilentlyContinue |
            ForEach-Object {
                $match = [regex]::Match($_.Name, "^state_(\d+)\.sqlite$")
                $index = if ($match.Success) { [int]$match.Groups[1].Value } else { -1 }
                [PSCustomObject]@{
                    Path = $_.FullName
                    Index = $index
                    LastWriteTimeUtc = $_.LastWriteTimeUtc
                }
            } |
            Sort-Object `
                @{ Expression = "Index"; Descending = $true }, `
                @{ Expression = "LastWriteTimeUtc"; Descending = $true }
    )

    if ($candidates.Count -eq 0) {
        return $null
    }

    return $candidates[0].Path
}

function Get-CodexProcessInventory {
    $currentPid = $PID
    return @(
        Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
            $_.ProcessId -ne $currentPid -and (
                $_.Name -eq "codex.exe" -or
                (
                    $_.CommandLine -and
                    $_.CommandLine -match "(?i)@openai\\\\codex|\\bcodex(\\.exe)?\\b"
                )
            )
        } | Sort-Object CreationDate
    )
}

function Get-SqliteCommandInfo {
    try {
        return Get-Command sqlite3 -ErrorAction Stop | Select-Object -First 1
    } catch {
        return $null
    }
}

function Get-IntegerFromText {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$Text
    )

    [int]$value = 0
    $trimmed = $Text.Trim()
    if (-not [int]::TryParse($trimmed, [ref]$value)) {
        throw "Expected integer output, got: $Text"
    }

    return $value
}

function ConvertTo-SqliteLiteral {
    param(
        [AllowEmptyString()]
        [string]$Value
    )

    return "'" + $Value.Replace("'", "''") + "'"
}

function Get-NormalizedCwdText {
    param(
        [AllowEmptyString()]
        [string]$Path
    )

    if ([string]::IsNullOrWhiteSpace($Path)) {
        return ""
    }

    $normalized = $Path.Trim()
    if ($normalized.StartsWith('\\?\')) {
        $normalized = $normalized.Substring(4)
    }

    return $normalized.Replace('/', '\')
}

function Get-StaleRowsWhereSql {
    param(
        [AllowEmptyString()]
        [string]$RepairCwd
    )

    $base = "has_user_event = 0 and trim(coalesce(first_user_message,'')) <> ''"
    $normalizedRepairCwd = Get-NormalizedCwdText -Path $RepairCwd
    if ([string]::IsNullOrWhiteSpace($normalizedRepairCwd)) {
        return $base
    }

    $repairCwdLiteral = ConvertTo-SqliteLiteral -Value $normalizedRepairCwd
    return "$base and lower(replace(cwd,'\\?\','')) = lower($repairCwdLiteral)"
}

function Invoke-ResumeStateRepair {
    param(
        [string]$StateDbPath,
        [string]$BackupRoot,
        [string]$RepairCwd
    )

    if ([string]::IsNullOrWhiteSpace($StateDbPath) -or -not (Test-Path -LiteralPath $StateDbPath)) {
        Write-WarnStep "State DB not found at $StateDbPath. Skipping DB repair."
        return
    }

    $sqliteCommand = Get-SqliteCommandInfo
    if (-not $sqliteCommand) {
        Write-WarnStep "sqlite3 was not found in PATH. Skipping DB repair."
        return
    }

    $sqliteExe = $sqliteCommand.Source
    $whereSql = Get-StaleRowsWhereSql -RepairCwd $RepairCwd
    $normalizedRepairCwd = Get-NormalizedCwdText -Path $RepairCwd
    if ([string]::IsNullOrWhiteSpace($normalizedRepairCwd)) {
        Write-Step "DB repair scope: all Codex thread rows."
    } else {
        Write-Step "Scoped DB repair cwd: $normalizedRepairCwd"
    }

    $staleCountText = & $sqliteExe $StateDbPath "select count(*) from threads where $whereSql;"
    $staleCount = Get-IntegerFromText -Text ($staleCountText | Select-Object -First 1)

    if ($staleCount -le 0) {
        Write-Step "threads.has_user_event looks healthy for the selected repair scope. No DB repair needed."
        return
    }

    if (-not (Test-Path -LiteralPath $BackupRoot)) {
        New-Item -ItemType Directory -Path $BackupRoot -Force | Out-Null
    }

    $timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $stateDbFileName = [System.IO.Path]::GetFileName($StateDbPath)
    $backupPath = Join-Path $BackupRoot ("$stateDbFileName.pre-has-user-event-heal-$timestamp.bak")
    & $sqliteExe $StateDbPath ".backup '$backupPath'" | Out-Null

    $updateText = & $sqliteExe $StateDbPath "PRAGMA busy_timeout = 5000; begin immediate; update threads set has_user_event = 1 where $whereSql; select changes(); commit;"
    $changedCount = Get-IntegerFromText -Text ($updateText | Select-Object -Last 1)

    $afterCountText = & $sqliteExe $StateDbPath "select count(*) from threads where $whereSql;"
    $afterCount = Get-IntegerFromText -Text ($afterCountText | Select-Object -First 1)

    Write-Step "Repaired threads.has_user_event drift. stale_before=$staleCount changed=$changedCount stale_after=$afterCount"
    Write-Step "SQLite backup: $backupPath"
}

function Show-MetamcpConfigDiagnostic {
    param(
        [string]$CodexHome
    )

    $configPath = Join-Path $CodexHome "config.toml"
    if (-not (Test-Path -LiteralPath $configPath)) {
        Write-Step "Codex config.toml not found. Skipping MetaMCP config diagnostics."
        return
    }

    $lines = [System.IO.File]::ReadAllLines($configPath)
    $inMetamcp = $false
    $foundMetamcp = $false
    $enabledValue = "<default true>"
    $startupTimeout = $null
    $endpoint = $null

    foreach ($line in $lines) {
        if ($line -match '^\[mcp_servers\.metamcp\]$') {
            $inMetamcp = $true
            $foundMetamcp = $true
            continue
        }

        if ($inMetamcp -and $line -match '^\[') {
            break
        }

        if ($inMetamcp -and $line -match '^\s*enabled\s*=') {
            $enabledValue = ($line -replace '^\s*enabled\s*=\s*', '').Trim()
            continue
        }

        if ($inMetamcp -and $line -match '^\s*startup_timeout_sec\s*=') {
            $startupTimeout = ($line -replace '^\s*startup_timeout_sec\s*=\s*', '').Trim()
            continue
        }

        if ($inMetamcp -and $line -match '["''](https?://[^"'']+)["'']') {
            $endpoint = $Matches[1]
        }
    }

    if (-not $foundMetamcp) {
        Write-Step "mcp_servers.metamcp is not configured."
        return
    }

    Write-Step "mcp_servers.metamcp enabled value: $enabledValue"
    if ($startupTimeout) {
        Write-Step "mcp_servers.metamcp startup_timeout_sec: $startupTimeout"
    }
    if ($endpoint) {
        Write-Step "mcp_servers.metamcp endpoint: $endpoint"
    } else {
        Write-WarnStep "mcp_servers.metamcp endpoint was not found in args."
    }
    Write-Step "Guardian preserves mcp_servers.metamcp. If native /resume stalls during MCP startup, repair the MetaMCP endpoint or child servers rather than disabling this config block."
}

function Get-DefaultModelProvider {
    param(
        [string]$CodexHome
    )

    $configPath = Join-Path $CodexHome "config.toml"
    if (-not (Test-Path -LiteralPath $configPath)) {
        return $null
    }

    foreach ($line in Get-Content -LiteralPath $configPath) {
        $trimmed = $line.Trim()
        if ([string]::IsNullOrWhiteSpace($trimmed) -or $trimmed.StartsWith("#")) {
            continue
        }
        $match = [regex]::Match($trimmed, "^model_provider\s*=\s*[""']([^""']+)[""']")
        if ($match.Success) {
            return $match.Groups[1].Value
        }
    }

    return $null
}

function Show-NativeResumeVisibility {
    param(
        [string]$StateDbPath,
        [string]$CodexHome,
        [string]$RepairCwd
    )

    if ([string]::IsNullOrWhiteSpace($StateDbPath) -or -not (Test-Path -LiteralPath $StateDbPath)) {
        return
    }

    $sqliteCommand = Get-SqliteCommandInfo
    if (-not $sqliteCommand) {
        return
    }

    $sqliteExe = $sqliteCommand.Source
    $defaultProvider = Get-DefaultModelProvider -CodexHome $CodexHome
    $displayProvider = Get-TextOrFallback -Value $defaultProvider -Fallback "<not found>"
    Write-Step "Default config model_provider: $displayProvider"
    Write-Step "Native /resume filter contract: archived=0, first_user_message non-empty, matching model_provider, and exact cwd while the picker is on Cwd filter."

    $knownProjects = @()
    $normalizedRepairCwd = Get-NormalizedCwdText -Path $RepairCwd
    if (-not [string]::IsNullOrWhiteSpace($normalizedRepairCwd)) {
        $knownProjects += $normalizedRepairCwd
    }
    $knownProjects += @(
        "D:\Desktop\Inkforge",
        "D:\Desktop\LawSaw",
        "D:\Desktop\CREATOR FOUR",
        "D:\Desktop\CREATOR SIX"
    )
    $knownProjects = @($knownProjects | Select-Object -Unique)
    foreach ($project in $knownProjects) {
        $escaped = $project.Replace("'", "''")
        $rows = & $sqliteExe -header -column $StateDbPath "with normalized as (select cwd, replace(cwd,'\\?\','') as norm_cwd, model_provider, archived, first_user_message, title, has_user_event from threads) select cwd, model_provider, count(*) as total, sum(case when archived=0 then 1 else 0 end) as active, sum(case when archived=0 and trim(coalesce(first_user_message,''))<>'' then 1 else 0 end) as native_visible, sum(case when archived=0 and trim(coalesce(first_user_message,''))='' and trim(coalesce(title,''))<>'' then 1 else 0 end) as title_only, sum(case when archived=0 and has_user_event=1 then 1 else 0 end) as has_user_event from normalized where lower(norm_cwd)=lower('$escaped') group by cwd, model_provider order by cwd, model_provider;"
        if ($rows) {
            Write-Step "Native visibility by exact cwd/provider for $project"
            $rows | ForEach-Object { Write-Step "  $_" }
        }
    }
}

function Install-ResumePickerWrapper {
    param(
        [string]$CodexHome
    )

    $npmGlobalRoot = $null
    try {
        $npmGlobalRoot = (& npm root -g 2>$null | Select-Object -First 1).Trim()
    } catch {
        Write-WarnStep "Unable to resolve npm global root. Skipping launcher wrapper install."
        return
    }

    if ([string]::IsNullOrWhiteSpace($npmGlobalRoot)) {
        Write-WarnStep "npm root -g returned an empty path. Skipping launcher wrapper install."
        return
    }

    $packageRoot = Join-Path $npmGlobalRoot "@openai\codex"
    $launcherPath = Join-Path $packageRoot "bin\codex.js"
    $backupPath = Join-Path $packageRoot "bin\codex.upstream.resume-fix.js"
    $helperPath = Join-Path $CodexHome "tools\codex-resume-picker.js"
    $metadataPath = Join-Path $CodexHome "tools\codex-resume-fix.json"

    if (-not (Test-Path -LiteralPath $launcherPath)) {
        Write-WarnStep "Codex launcher not found at $launcherPath. Skipping launcher wrapper install."
        return
    }

    if (-not (Test-Path -LiteralPath $helperPath)) {
        Write-WarnStep "Resume picker helper not found at $helperPath. Skipping launcher wrapper install."
        return
    }

    $launcherContent = Get-Content -LiteralPath $launcherPath -Raw -Encoding UTF8
    $alreadyPatched = $launcherContent.Contains("codex.upstream.resume-fix.js")
    $wrapperCurrent = $alreadyPatched -and $launcherContent.Contains("pickerOnlyFlags") -and $launcherContent.Contains("--max-visible") -and -not $launcherContent.Contains("CODEX_NATIVE_HOTFIX")

    if (-not $alreadyPatched) {
        Copy-Item -LiteralPath $launcherPath -Destination $backupPath -Force
    } elseif (-not (Test-Path -LiteralPath $backupPath)) {
        Write-WarnStep "Codex launcher is already wrapped but upstream backup is missing: $backupPath"
        return
    }

    if (-not $wrapperCurrent) {
        $wrapperContent = @'
#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath, pathToFileURL } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const args = process.argv.slice(2);
const isResumeCommand = args[0] === 'resume';
const resumeArgs = args.slice(1);
const pickerOnlyFlags = new Set(['--all', '--no-alt-screen', '--include-non-interactive']);
const hasNativeResumeTarget =
  resumeArgs.includes('--last') ||
  resumeArgs.some((arg) => !arg.startsWith('-')) ||
  resumeArgs.some((arg) => !pickerOnlyFlags.has(arg));
const shouldInterceptResume =
  isResumeCommand &&
  !hasNativeResumeTarget;

const userHome = process.env.USERPROFILE || process.env.HOME || '';
const helperPath =
  process.env.CODEX_RESUME_FIX_HELPER ||
  path.join(userHome, '.codex', 'tools', 'codex-resume-picker.js');
const upstreamPath = path.join(__dirname, 'codex.upstream.resume-fix.js');

function exitWith(result) {
  process.exit(typeof result.status === 'number' ? result.status : 1);
}

if (shouldInterceptResume && existsSync(helperPath)) {
  const helperArgs = [helperPath, '--pick', '--limit', '50', '--max-visible'];

  const result = spawnSync(process.execPath, helperArgs, {
    stdio: 'inherit',
    env: process.env,
  });

  exitWith(result);
}

if (!existsSync(upstreamPath)) {
  console.error(`Missing upstream Codex launcher backup: ${upstreamPath}`);
  console.error('Rerun the Codex resume fix installer to repair the launcher.');
  process.exit(1);
}

await import(pathToFileURL(upstreamPath).href);
'@

        [System.IO.File]::WriteAllText(
            $launcherPath,
            $wrapperContent,
            [System.Text.UTF8Encoding]::new($false)
        )
        Write-Step "Installed Codex resume picker launcher wrapper."
    } else {
        Write-Step "Codex resume picker launcher wrapper already installed."
    }

    $metadata = @{
        installed_at = (Get-Date).ToString("s")
        npm_global_root = $npmGlobalRoot
        package_root = $packageRoot
        launcher_path = $launcherPath
        backup_path = $backupPath
        helper_path = $helperPath
        native_hotfix_present = $false
        native_hotfix_note = "Disabled: launcher must preserve the installed Codex CLI version and delegate non-picker commands to upstream."
        repair_script = $PSCommandPath
    } | ConvertTo-Json -Depth 4

    Set-Content -LiteralPath $metadataPath -Value $metadata -Encoding UTF8
    Write-Step "Resume picker metadata: $metadataPath"
}

function Assert-HealthyVersion {
    param(
        [version]$Version,
        [string]$VersionText,
        [version]$MinimumVersion
    )

    if (-not $Version) {
        throw "Unable to read codex version. Raw output: $VersionText"
    }

    if ($Version -lt $MinimumVersion) {
        throw "Repair failed: codex version is still below $MinimumVersion ($VersionText)."
    }
}

function Show-VerificationGuide {
    param(
        [int]$RelatedProcessCount
    )

    Write-Step "Suggested interactive verification:"
    Write-Step "  1. Run: codex --no-alt-screen"
    Write-Step "  2. If 'Starting MCP servers...' is still visible, press Esc once."
    Write-Step "  3. Type: /resume"
    Write-Step "  4. Press Enter."
    Write-Step "Important: the external fallback picker below is not proof that the in-app slash picker works; it only checks the launcher wrapper path."
    Write-Step "  codex resume --all --no-alt-screen"

    if ($RelatedProcessCount -gt 0) {
        Write-Step "Note: detected $RelatedProcessCount Codex-related process(es). This script does not stop them."
        Write-Step "If npm shows EPERM cleanup warnings but 'codex --version' is already healthy, you can ignore the cleanup warning."
    }
}

$codexCommand = Get-CodexCommandInfo
$beforeText = Get-CodexVersionText
$beforeVersion = Get-CodexVersion -VersionText $beforeText
$displayBeforeText = Get-TextOrFallback -Value $beforeText -Fallback "<not found>"
$commandPath = if ($codexCommand) { $codexCommand.Source } else { "<not found>" }

$TargetVersionObject = $null
if (-not [string]::IsNullOrWhiteSpace($TargetVersion)) {
    $TargetVersionObject = [version]$TargetVersion
} elseif ($beforeVersion) {
    $TargetVersion = $beforeVersion.ToString()
    $TargetVersionObject = $beforeVersion
}

$historyPath = Join-Path $CodexHome "history.jsonl"
$sessionsRoot = Join-Path $CodexHome "sessions"
if ([string]::IsNullOrWhiteSpace($StateDbPath)) {
    $StateDbPath = Get-LatestStateDbPath -CodexHome $CodexHome
}

$historyLineCount = Get-FileLineCount -Path $historyPath
$rolloutFileCount = Get-RolloutFileCount -SessionsRoot $sessionsRoot
$stateDbDisplay = Get-TextOrFallback -Value $StateDbPath -Fallback "<not found>"
$stateDbExists = if ([string]::IsNullOrWhiteSpace($StateDbPath)) {
    $false
} else {
    Test-Path -LiteralPath $StateDbPath
}
$processInventory = Get-CodexProcessInventory
$processCount = @($processInventory).Count

Write-Step "codex command: $commandPath"
Write-Step "Current version: $displayBeforeText"
Write-Step "Codex home: $CodexHome"
Write-Step "history.jsonl lines: $historyLineCount"
Write-Step "rollout jsonl files: $rolloutFileCount"
Write-Step "selected state db: $stateDbDisplay"
Write-Step "state db present: $stateDbExists"
Write-Step "Detected Codex-related processes: $processCount"

if ($processCount -gt 0) {
    $processInventory |
        Select-Object -First 5 ProcessId, Name, CreationDate |
        ForEach-Object {
            Write-Step ("  PID {0} {1} started {2}" -f $_.ProcessId, $_.Name, $_.CreationDate)
        }
}

if ($historyLineCount -eq 0 -and $rolloutFileCount -eq 0) {
    Write-WarnStep "No history/session files were detected under $CodexHome. CLI repair may succeed even if stored sessions are currently empty."
}

$needsInstall = $false
if ($ForceInstall) {
    if (-not $TargetVersionObject) {
        throw "ForceInstall requires -TargetVersion when the current codex version cannot be detected."
    }
    $needsInstall = $true
} elseif ($TargetVersionObject) {
    $needsInstall = -not $beforeVersion -or ($beforeVersion -lt $TargetVersionObject)
}

if ($SkipInstall) {
    Write-Step "SkipInstall requested. Detection-only mode."
} elseif ($needsInstall) {
    if ($ForceInstall) {
        Write-Step "ForceInstall requested. Installing @openai/codex@$TargetVersion ..."
    } else {
        Write-Step "Upgrading to @openai/codex@$TargetVersion ..."
    }

    npm install -g "@openai/codex@$TargetVersion"
} else {
    if ($TargetVersionObject) {
        Write-Step "Version already >= $TargetVersion. No upgrade needed."
    } else {
        Write-Step "No target version provided. Skipping install step and validating the current Codex state only."
    }
}

$afterText = Get-CodexVersionText
$afterVersion = Get-CodexVersion -VersionText $afterText

if ($TargetVersionObject) {
    Assert-HealthyVersion -Version $afterVersion -VersionText $afterText -MinimumVersion $TargetVersionObject
} elseif (-not $afterVersion) {
    throw "Unable to read codex version after repair. Provide -TargetVersion or make sure codex is available in PATH."
}

Invoke-ResumeStateRepair -StateDbPath $StateDbPath -BackupRoot (Join-Path $CodexHome "backups") -RepairCwd $RepairCwd
Show-MetamcpConfigDiagnostic -CodexHome $CodexHome
Show-NativeResumeVisibility -StateDbPath $StateDbPath -CodexHome $CodexHome -RepairCwd $RepairCwd
Install-ResumePickerWrapper -CodexHome $CodexHome

Write-Step "Repair complete. Active version: $afterText"
Show-VerificationGuide -RelatedProcessCount $processCount
