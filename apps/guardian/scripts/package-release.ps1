[CmdletBinding()]
param(
    [string]$Version = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Guardian releases assume the user has already installed `@openai/codex`
# (the prerequisite for the tool's existence). The release zip therefore
# ships ONLY guardian.exe + docs + the embedded repair script. The slow-path
# (C4) launcher patch falls back to the user's own `vendor/<triple>/codex/codex.exe`
# when no `vendor-hotfix/<triple>/codex/codex.exe` is staged, so we deliberately
# do not bundle a copy of `codex.exe`.

function Get-RepoRoot {
    return (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
}

function Get-DefaultVersion {
    param(
        [string]$RepoRoot
    )

    $metadataJson = cargo metadata --no-deps --format-version 1 --manifest-path (Join-Path $RepoRoot "Cargo.toml")
    $metadata = $metadataJson | ConvertFrom-Json
    $guardian = $metadata.packages | Where-Object { $_.name -eq "guardian" } | Select-Object -First 1
    if (-not $guardian) {
        throw "Unable to resolve guardian package metadata."
    }

    return "v{0}" -f $guardian.version
}

function Write-ChecksumFile {
    param(
        [string]$ArtifactDirectory,
        [string[]]$Files
    )

    $checksumPath = Join-Path $ArtifactDirectory "SHA256SUMS.txt"
    $lines = @()

    foreach ($file in $Files) {
        $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
        $name = [System.IO.Path]::GetFileName($file)
        $lines += "{0}  {1}" -f $hash, $name
    }

    Set-Content -LiteralPath $checksumPath -Value $lines -Encoding ascii
    return $checksumPath
}

$repoRoot = Get-RepoRoot
Push-Location $repoRoot

try {
    if ([string]::IsNullOrWhiteSpace($Version)) {
        $Version = Get-DefaultVersion -RepoRoot $repoRoot
    }

    $artifactRoot = Join-Path $repoRoot ("dist\{0}" -f $Version)
    $stagingRoot = Join-Path $artifactRoot "staging"
    $zipName = "guardian-{0}-windows-x64.zip" -f $Version
    $zipPath = Join-Path $artifactRoot $zipName
    $releaseExe = Join-Path $repoRoot "target\release\guardian.exe"

    if (Test-Path -LiteralPath $artifactRoot) {
        Remove-Item -LiteralPath $artifactRoot -Recurse -Force
    }

    New-Item -ItemType Directory -Path $stagingRoot -Force | Out-Null

    cargo build --release -p guardian

    Copy-Item -LiteralPath $releaseExe -Destination (Join-Path $artifactRoot "guardian.exe")
    Copy-Item -LiteralPath $releaseExe -Destination (Join-Path $stagingRoot "guardian.exe")
    Copy-Item -LiteralPath (Join-Path $repoRoot "README.md") -Destination (Join-Path $stagingRoot "README.md")
    Copy-Item -LiteralPath (Join-Path $repoRoot "CHANGELOG.md") -Destination (Join-Path $stagingRoot "CHANGELOG.md")
    Copy-Item -LiteralPath (Join-Path $repoRoot "LICENSE") -Destination (Join-Path $stagingRoot "LICENSE")

    $bundledRepairScript = Join-Path $repoRoot "apps\guardian\assets\tools\repair-codex-resume.ps1"
    if (-not (Test-Path -LiteralPath $bundledRepairScript)) {
        throw "Bundled repair script missing at $bundledRepairScript; cannot package release."
    }
    $stagingToolsDir = Join-Path $stagingRoot "tools"
    New-Item -ItemType Directory -Path $stagingToolsDir -Force | Out-Null
    Copy-Item -LiteralPath $bundledRepairScript -Destination (Join-Path $stagingToolsDir "repair-codex-resume.ps1")

    if (Test-Path -LiteralPath $zipPath) {
        Remove-Item -LiteralPath $zipPath -Force
    }

    Compress-Archive -Path (Join-Path $stagingRoot "*") -DestinationPath $zipPath -CompressionLevel Optimal

    $checksumPath = Write-ChecksumFile -ArtifactDirectory $artifactRoot -Files @($zipPath)

    Write-Host "Packaged release assets:"
    Write-Host "  Version: $Version"
    Write-Host "  Local smoke EXE: $(Join-Path $artifactRoot 'guardian.exe')"
    Write-Host "  Release ZIP: $zipPath"
    Write-Host "  Checksums: $checksumPath"
}
finally {
    Pop-Location
}
