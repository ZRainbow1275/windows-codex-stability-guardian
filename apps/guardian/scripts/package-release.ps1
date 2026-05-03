[CmdletBinding()]
param(
    [string]$Version = "",
    [string]$HotfixBinary = "",
    [string]$HotfixSha256 = "927ece82f53d23383fc70b21d3b3c35fc024e0bfae76bc548f98f9295cad2c89"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

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

function Get-HotfixCandidatePaths {
    param(
        [string]$RepoRoot,
        [string]$TargetTriple = "x86_64-pc-windows-msvc"
    )

    $relative = Join-Path -Path "vendor-hotfix" -ChildPath (Join-Path $TargetTriple "codex\codex.exe")
    $candidates = @()

    # 1. Maintainer-staged repo cache (preferred — gitignored, deterministic).
    $candidates += Join-Path $RepoRoot $relative

    # 2. Most recent prior `dist/v*/...` packaged release on the same machine.
    $distRoot = Join-Path $RepoRoot "dist"
    if (Test-Path -LiteralPath $distRoot) {
        $priorCopies = Get-ChildItem -LiteralPath $distRoot -Directory -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending |
            ForEach-Object { Join-Path $_.FullName $relative } |
            Where-Object { Test-Path -LiteralPath $_ }
        $candidates += $priorCopies
    }

    # 3. NPM-installed @openai/codex package on this machine (where prior Guardian runs staged it).
    if ($env:APPDATA) {
        $candidates += Join-Path $env:APPDATA ("npm\node_modules\@openai\codex\" + $relative)
    }

    # 4. Local Codex source build (existing pre-0.1.2 convention).
    if ($env:TEMP) {
        $candidates += Join-Path $env:TEMP "codex-src\codex-rs\target\release\codex.exe"
    }

    return $candidates
}

function Resolve-HotfixBinary {
    param(
        [string]$RepoRoot,
        [string]$ExplicitPath,
        [string]$ExpectedSha256
    )

    if (-not [string]::IsNullOrWhiteSpace($ExplicitPath)) {
        if (-not (Test-Path -LiteralPath $ExplicitPath)) {
            throw "Explicit -HotfixBinary '$ExplicitPath' not found."
        }
        return [pscustomobject]@{ Path = (Resolve-Path -LiteralPath $ExplicitPath).Path; Source = "explicit" }
    }

    foreach ($candidate in (Get-HotfixCandidatePaths -RepoRoot $RepoRoot)) {
        if (Test-Path -LiteralPath $candidate) {
            $resolved = (Resolve-Path -LiteralPath $candidate).Path
            if ([string]::IsNullOrWhiteSpace($ExpectedSha256)) {
                return [pscustomobject]@{ Path = $resolved; Source = "auto" }
            }

            $actual = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($actual -eq $ExpectedSha256.ToLowerInvariant()) {
                return [pscustomobject]@{ Path = $resolved; Source = "auto"; Sha256 = $actual }
            }

            Write-Warning ("Hotfix candidate '{0}' SHA256 mismatch (expected {1}, got {2}); continuing search." -f $resolved, $ExpectedSha256, $actual)
        }
    }

    return $null
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
    $hotfixRelativePath = "vendor-hotfix\x86_64-pc-windows-msvc\codex\codex.exe"
    $hotfixResolution = Resolve-HotfixBinary -RepoRoot $repoRoot -ExplicitPath $HotfixBinary -ExpectedSha256 $HotfixSha256
    $bundledHotfixSource = if ($hotfixResolution) { $hotfixResolution.Path } else { $null }

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

    if ($bundledHotfixSource) {
        $artifactHotfixPath = Join-Path $artifactRoot $hotfixRelativePath
        $stagingHotfixPath = Join-Path $stagingRoot $hotfixRelativePath
        New-Item -ItemType Directory -Path ([System.IO.Path]::GetDirectoryName($artifactHotfixPath)) -Force | Out-Null
        New-Item -ItemType Directory -Path ([System.IO.Path]::GetDirectoryName($stagingHotfixPath)) -Force | Out-Null
        Copy-Item -LiteralPath $bundledHotfixSource -Destination $artifactHotfixPath
        Copy-Item -LiteralPath $bundledHotfixSource -Destination $stagingHotfixPath
        Write-Host ("  Hotfix source: {0}" -f $bundledHotfixSource)
        if ($hotfixResolution.PSObject.Properties['Sha256']) {
            Write-Host ("  Hotfix SHA256: {0}" -f $hotfixResolution.Sha256)
        }
    } else {
        $checkedPaths = (Get-HotfixCandidatePaths -RepoRoot $repoRoot) -join "; "
        Write-Warning ("No verified Codex hotfix binary found (expected SHA256 {0}). Searched: {1}. Stage one at vendor-hotfix\x86_64-pc-windows-msvc\codex\codex.exe in the repo root, pass -HotfixBinary <path>, or override -HotfixSha256." -f $HotfixSha256, $checkedPaths)
    }

    if (Test-Path -LiteralPath $zipPath) {
        Remove-Item -LiteralPath $zipPath -Force
    }

    Compress-Archive -Path (Join-Path $stagingRoot "*") -DestinationPath $zipPath -CompressionLevel Optimal

    $checksumPath = Write-ChecksumFile -ArtifactDirectory $artifactRoot -Files @(
        (Join-Path $artifactRoot "guardian.exe"),
        $zipPath
    )

    Write-Host "Packaged release assets:"
    Write-Host "  Version: $Version"
    Write-Host "  EXE: $(Join-Path $artifactRoot 'guardian.exe')"
    Write-Host "  ZIP: $zipPath"
    if ($bundledHotfixSource) {
        Write-Host "  Bundled hotfix: $(Join-Path $artifactRoot $hotfixRelativePath)"
    } else {
        Write-Host "  Bundled hotfix: not included (no local hotfix source found)"
    }
    Write-Host "  Checksums: $checksumPath"
}
finally {
    Pop-Location
}
