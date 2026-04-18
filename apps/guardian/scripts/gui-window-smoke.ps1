param(
    [string]$GuardianExe,
    [int]$StartupTimeoutSeconds = 30,
    [int]$ActionTimeoutSeconds = 60
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class GuardianGuiNative {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    public const int BM_CLICK = 0x00F5;
    public const int WM_CLOSE = 0x0010;

    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetClassNameW(IntPtr hWnd, StringBuilder text, int maxCount);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowTextLengthW(IntPtr hWnd);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder text, int maxCount);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern IntPtr GetDlgItem(IntPtr hWnd, int nIDDlgItem);

    [DllImport("user32.dll")]
    public static extern bool IsWindowEnabled(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern IntPtr SendMessageW(IntPtr hWnd, int msg, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool PostMessageW(IntPtr hWnd, int msg, IntPtr wParam, IntPtr lParam);
}
"@

function Write-Step {
    param([string]$Message)

    Write-Host ("[{0}] {1}" -f (Get-Date -Format "HH:mm:ss"), $Message)
}

function Wait-Until {
    param(
        [string]$Description,
        [scriptblock]$Condition,
        [int]$TimeoutSeconds = 15,
        [int]$PollMilliseconds = 25
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $result = & $Condition
        if ($result) {
            return $result
        }
        Start-Sleep -Milliseconds $PollMilliseconds
    }

    throw "Timed out waiting for: $Description"
}

function Wait-ForGuardianExit {
    param(
        [int]$ProcessId,
        [int]$TimeoutSeconds
    )

    try {
        Wait-Process -Id $ProcessId -Timeout $TimeoutSeconds -ErrorAction Stop
    } catch {
        if (-not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) {
            return
        }

        throw
    }
}

function Get-WindowText {
    param([IntPtr]$Hwnd)

    if ($Hwnd -eq [IntPtr]::Zero) {
        return ""
    }

    $length = [GuardianGuiNative]::GetWindowTextLengthW($Hwnd)
    $builder = New-Object System.Text.StringBuilder ($length + 16)
    [void][GuardianGuiNative]::GetWindowTextW($Hwnd, $builder, $builder.Capacity)
    return $builder.ToString()
}

function Find-GuardianGuiWindow {
    param([uint32]$TargetProcessId)

    $script:__guardian_gui_window = [IntPtr]::Zero
    $callback = [GuardianGuiNative+EnumWindowsProc]{
        param([IntPtr]$Hwnd, [IntPtr]$LParam)

        if (-not [GuardianGuiNative]::IsWindowVisible($Hwnd)) {
            return $true
        }

        $windowPid = 0
        [void][GuardianGuiNative]::GetWindowThreadProcessId($Hwnd, [ref]$windowPid)
        if ($windowPid -ne $TargetProcessId) {
            return $true
        }

        $className = New-Object System.Text.StringBuilder 256
        [void][GuardianGuiNative]::GetClassNameW($Hwnd, $className, $className.Capacity)
        if ($className.ToString() -ne "GuardianGuiWindow") {
            return $true
        }

        $script:__guardian_gui_window = $Hwnd
        return $false
    }

    [void][GuardianGuiNative]::EnumWindows($callback, [IntPtr]::Zero)
    return $script:__guardian_gui_window
}

function Require-ControlHandle {
    param(
        [IntPtr]$ParentWindow,
        [int]$ControlId,
        [string]$Label
    )

    $handle = [GuardianGuiNative]::GetDlgItem($ParentWindow, $ControlId)
    if ($handle -eq [IntPtr]::Zero) {
        throw "failed to resolve GUI control '$Label' (id=$ControlId)."
    }

    return $handle
}

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\..\.."))
if (-not $GuardianExe) {
    $GuardianExe = Join-Path $repoRoot "target\debug\guardian.exe"
}

if (-not (Test-Path -LiteralPath $GuardianExe)) {
    throw "guardian executable not found: $GuardianExe"
}

$process = $null

try {
    Write-Step "starting guardian gui"
    $process = Start-Process -FilePath $GuardianExe -ArgumentList @("gui") -PassThru
    $targetProcessId = [uint32]$process.Id

    $windowHandle = Wait-Until -Description "Guardian GUI main window" -TimeoutSeconds $StartupTimeoutSeconds -Condition {
        $window = Find-GuardianGuiWindow -TargetProcessId $targetProcessId
        if ($window -eq [IntPtr]::Zero) {
            return $false
        }

        return $window
    }

    $stepDiagnoseButton = Require-ControlHandle -ParentWindow $windowHandle -ControlId 1505 -Label "步骤 2：开始诊断"
    $stepExportButton = Require-ControlHandle -ParentWindow $windowHandle -ControlId 1507 -Label "步骤 4：导出证据"
    $runCheckButton = Require-ControlHandle -ParentWindow $windowHandle -ControlId 1001 -Label "刷新整机检查"
    $diagnoseProfileButton = Require-ControlHandle -ParentWindow $windowHandle -ControlId 1004 -Label "只读诊断 Profile"
    $exportBundleZipButton = Require-ControlHandle -ParentWindow $windowHandle -ControlId 1006 -Label "导出并压缩"
    $openLatestBundleZipButton = Require-ControlHandle -ParentWindow $windowHandle -ControlId 1009 -Label "打开最新压缩包"

    $initialBusyTitle = Wait-Until -Description "startup busy title" -TimeoutSeconds $StartupTimeoutSeconds -Condition {
        $title = Get-WindowText -Hwnd $windowHandle
        if ($title -like "Guardian 稳定性控制台 - * - 正在执行：刷新整机检查") {
            return $title
        }

        return $false
    }
    Write-Step ("startup title confirmed: {0}" -f $initialBusyTitle)

    $initialRunningLabel = Wait-Until -Description "Run Check running label" -TimeoutSeconds $StartupTimeoutSeconds -Condition {
        $text = Get-WindowText -Hwnd $runCheckButton
        if ($text -eq "刷新整机检查（执行中）") {
            return $text
        }

        return $false
    }
    Write-Step ("startup button confirmed: {0}" -f $initialRunningLabel)

    $openZipEnabledBeforeExport = [GuardianGuiNative]::IsWindowEnabled($openLatestBundleZipButton)
    if ($openZipEnabledBeforeExport) {
        throw "Open Latest Bundle Zip should be disabled before the first zip export."
    }
    Write-Step "confirmed latest zip button is disabled before export"

    $startupSettled = Wait-Until -Description "startup settled state" -TimeoutSeconds $StartupTimeoutSeconds -Condition {
        $title = Get-WindowText -Hwnd $windowHandle
        $runCheckText = Get-WindowText -Hwnd $runCheckButton
        $exportEnabled = [GuardianGuiNative]::IsWindowEnabled($exportBundleZipButton)

        if (($title -notlike "*正在执行：*") -and ($runCheckText -eq "刷新整机检查") -and $exportEnabled) {
            return [pscustomobject]@{
                Title = $title
                RunCheckText = $runCheckText
            }
        }

        return $false
    }
    Write-Step ("startup settled: {0}" -f $startupSettled.Title)

    Write-Step "switching to 开始诊断 step"
    [void][GuardianGuiNative]::SendMessageW(
        $stepDiagnoseButton,
        [GuardianGuiNative]::BM_CLICK,
        [IntPtr]::Zero,
        [IntPtr]::Zero
    )
    Wait-Until -Description "diagnose step visibility" -TimeoutSeconds $StartupTimeoutSeconds -Condition {
        if ([GuardianGuiNative]::IsWindowVisible($diagnoseProfileButton) -and [GuardianGuiNative]::IsWindowEnabled($diagnoseProfileButton)) {
            return $true
        }

        return $false
    } | Out-Null

    Write-Step "clicking 只读诊断 Profile"
    [void][GuardianGuiNative]::SendMessageW(
        $diagnoseProfileButton,
        [GuardianGuiNative]::BM_CLICK,
        [IntPtr]::Zero,
        [IntPtr]::Zero
    )

    $diagnoseSettled = Wait-Until -Description "diagnose settled state" -TimeoutSeconds $ActionTimeoutSeconds -Condition {
        $title = Get-WindowText -Hwnd $windowHandle
        $buttonText = Get-WindowText -Hwnd $diagnoseProfileButton

        if (($title -notlike "*正在执行：*") -and ($buttonText -eq "只读诊断 Profile")) {
            return [pscustomobject]@{
                Title = $title
                ButtonText = $buttonText
            }
        }

        return $false
    }
    Write-Step ("diagnose settled: {0}" -f $diagnoseSettled.Title)

    Write-Step "switching to 导出证据 step"
    [void][GuardianGuiNative]::SendMessageW(
        $stepExportButton,
        [GuardianGuiNative]::BM_CLICK,
        [IntPtr]::Zero,
        [IntPtr]::Zero
    )
    Wait-Until -Description "export step visibility" -TimeoutSeconds $StartupTimeoutSeconds -Condition {
        if ([GuardianGuiNative]::IsWindowVisible($exportBundleZipButton) -and [GuardianGuiNative]::IsWindowEnabled($exportBundleZipButton)) {
            return $true
        }

        return $false
    } | Out-Null

    Write-Step "clicking 导出并压缩"
    [void][GuardianGuiNative]::SendMessageW(
        $exportBundleZipButton,
        [GuardianGuiNative]::BM_CLICK,
        [IntPtr]::Zero,
        [IntPtr]::Zero
    )

    $exportBusy = $null
    $exportSettled = $null
    $exportDeadline = (Get-Date).AddSeconds($ActionTimeoutSeconds)
    while ((Get-Date) -lt $exportDeadline) {
        $title = Get-WindowText -Hwnd $windowHandle
        $buttonText = Get-WindowText -Hwnd $exportBundleZipButton
        $openZipEnabled = [GuardianGuiNative]::IsWindowEnabled($openLatestBundleZipButton)

        if (($null -eq $exportBusy) -and ($title -like "Guardian 稳定性控制台 - * - 正在执行：导出并压缩") -and ($buttonText -eq "导出并压缩（执行中）")) {
            $exportBusy = [pscustomobject]@{
                Title = $title
                ButtonText = $buttonText
            }
            Write-Step ("export busy title confirmed: {0}" -f $exportBusy.Title)
        }

        if (($title -notlike "*正在执行：*") -and ($buttonText -eq "导出并压缩") -and $openZipEnabled) {
            $exportSettled = [pscustomobject]@{
                Title = $title
                ButtonText = $buttonText
                OpenZipEnabled = $openZipEnabled
            }
            break
        }

        Start-Sleep -Milliseconds 25
    }

    if ($null -eq $exportSettled) {
        throw "Timed out waiting for the export settled state."
    }
    if ($null -eq $exportBusy) {
        Write-Step "export completed before the busy transition could be sampled"
    }
    Write-Step ("export settled: {0}" -f $exportSettled.Title)

    Write-Step "closing guardian gui"
    [void][GuardianGuiNative]::PostMessageW(
        $windowHandle,
        [GuardianGuiNative]::WM_CLOSE,
        [IntPtr]::Zero,
        [IntPtr]::Zero
    )
    Wait-ForGuardianExit -ProcessId $process.Id -TimeoutSeconds 10

    $result = [pscustomobject]@{
        process_id = $process.Id
        initial_busy_title = $initialBusyTitle
        initial_run_check_label = $initialRunningLabel
        open_latest_zip_enabled_before_export = $openZipEnabledBeforeExport
        startup_settled_title = $startupSettled.Title
        export_busy_title = $exportBusy.Title
        export_busy_button = $exportBusy.ButtonText
        export_settled_title = $exportSettled.Title
        open_latest_zip_enabled_after_export = $exportSettled.OpenZipEnabled
    }

    Write-Step "guardian gui smoke passed"
    $result | ConvertTo-Json -Depth 4
    exit 0
}
finally {
    if ($process -and (Get-Process -Id $process.Id -ErrorAction SilentlyContinue)) {
        Stop-Process -Id $process.Id -Force
    }
}
