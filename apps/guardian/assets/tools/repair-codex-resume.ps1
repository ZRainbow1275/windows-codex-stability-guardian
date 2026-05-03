[CmdletBinding()]
param(
    [string]$TargetVersion = "",
    [string]$CodexHome = "",
    [string]$StateDbPath = "",
    [switch]$SkipInstall,
    [switch]$ForceInstall
)

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

function Invoke-ResumeStateRepair {
    param(
        [string]$StateDbPath,
        [string]$BackupRoot
    )

    if (-not (Test-Path -LiteralPath $StateDbPath)) {
        Write-WarnStep "State DB not found at $StateDbPath. Skipping DB repair."
        return
    }

    $sqliteCommand = Get-SqliteCommandInfo
    if (-not $sqliteCommand) {
        Write-WarnStep "sqlite3 was not found in PATH. Skipping DB repair."
        return
    }

    $sqliteExe = $sqliteCommand.Source
    $staleCountText = & $sqliteExe $StateDbPath "select count(*) from threads where has_user_event = 0 and trim(first_user_message) <> '';"
    $staleCount = Get-IntegerFromText -Text ($staleCountText | Select-Object -First 1)

    if ($staleCount -le 0) {
        Write-Step "threads.has_user_event looks healthy. No DB repair needed."
        return
    }

    if (-not (Test-Path -LiteralPath $BackupRoot)) {
        New-Item -ItemType Directory -Path $BackupRoot -Force | Out-Null
    }

    $timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $stateDbFileName = [System.IO.Path]::GetFileName($StateDbPath)
    $backupPath = Join-Path $BackupRoot ("$stateDbFileName.pre-has-user-event-heal-$timestamp.bak")
    & $sqliteExe $StateDbPath ".backup '$backupPath'" | Out-Null

    $updateText = & $sqliteExe $StateDbPath "PRAGMA busy_timeout = 5000; begin immediate; update threads set has_user_event = 1 where has_user_event = 0 and trim(first_user_message) <> ''; select changes(); commit;"
    $changedCount = Get-IntegerFromText -Text ($updateText | Select-Object -Last 1)

    $afterCountText = & $sqliteExe $StateDbPath "select count(*) from threads where has_user_event = 0 and trim(first_user_message) <> '';"
    $afterCount = Get-IntegerFromText -Text ($afterCountText | Select-Object -First 1)

    Write-Step "Repaired threads.has_user_event drift. stale_before=$staleCount changed=$changedCount stale_after=$afterCount"
    Write-Step "SQLite backup: $backupPath"
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
    Write-Step "Fallback picker check:"
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

Invoke-ResumeStateRepair -StateDbPath $StateDbPath -BackupRoot (Join-Path $CodexHome "backups")

Write-Step "Repair complete. Active version: $afterText"
Show-VerificationGuide -RelatedProcessCount $processCount
