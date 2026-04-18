param(
    [string]$GuardianExe,
    [int]$StartupTimeoutSeconds = 30,
    [int]$ActionTimeoutSeconds = 30
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.IO.Compression.FileSystem

Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class GuardianTrayNative {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    public const uint MOUSEEVENTF_RIGHTDOWN = 0x0008;
    public const uint MOUSEEVENTF_RIGHTUP = 0x0010;
    public const int WM_USER_TRAYICON = 6002;
    public const int WM_RBUTTONDOWN = 0x0204;
    public const int WM_COMMAND = 0x0111;
    public const int WM_CANCELMODE = 0x001F;
    public const int WM_KEYDOWN = 0x0100;
    public const int WM_KEYUP = 0x0101;
    public const int VK_ESCAPE = 0x1B;
    public const int MN_GETHMENU = 0x01E1;

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool SetCursorPos(int x, int y);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extraInfo);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool PostMessageW(IntPtr hwnd, int msg, UIntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr SendMessageW(IntPtr hwnd, int msg, UIntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool IsWindowVisible(IntPtr hwnd);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetClassNameW(IntPtr hwnd, StringBuilder text, int maxCount);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetMenuStringW(IntPtr hMenu, uint item, StringBuilder text, int maxCount, uint flags);

    [DllImport("user32.dll")]
    public static extern int GetMenuItemCount(IntPtr hMenu);

    [DllImport("user32.dll")]
    public static extern uint GetMenuItemID(IntPtr hMenu, int position);

    [DllImport("user32.dll")]
    public static extern uint GetMenuState(IntPtr hMenu, uint item, uint flags);

    [DllImport("user32.dll")]
    public static extern IntPtr GetSubMenu(IntPtr hMenu, int nPos);
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
        [int]$PollMilliseconds = 250
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

function Read-TestState {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return $null
    }

    try {
        return Get-Content -LiteralPath $Path -Raw -Encoding utf8 | ConvertFrom-Json
    } catch {
        return $null
    }
}

function Close-OpenTrayMenu {
    [System.Windows.Forms.SendKeys]::SendWait("{ESC}")
    Start-Sleep -Milliseconds 250
}

function Invoke-RightClickAt {
    param(
        [int]$X,
        [int]$Y
    )

    [GuardianTrayNative]::SetCursorPos($X, $Y) | Out-Null
    Start-Sleep -Milliseconds 120
    [GuardianTrayNative]::mouse_event([GuardianTrayNative]::MOUSEEVENTF_RIGHTDOWN, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 80
    [GuardianTrayNative]::mouse_event([GuardianTrayNative]::MOUSEEVENTF_RIGHTUP, 0, 0, 0, [UIntPtr]::Zero)
}

function Open-TrayMenuWithShellClick {
    param([object]$State)

    if (-not $State.tray_rect) {
        throw "Tray state does not contain a tray_rect."
    }

    $centerX = [int][Math]::Round([double]$State.tray_rect.x + ([double]$State.tray_rect.width / 2.0))
    $centerY = [int][Math]::Round([double]$State.tray_rect.y + ([double]$State.tray_rect.height / 2.0))

    Invoke-RightClickAt -X $centerX -Y $centerY
    Start-Sleep -Milliseconds 300
}

function Open-TrayMenuWithWindowMessage {
    param([object]$State)

    if (-not $State.tray_rect) {
        throw "Tray state does not contain a tray_rect."
    }
    if (-not $State.tray_hwnd) {
        throw "Tray state does not contain tray_hwnd."
    }

    $centerX = [int][Math]::Round([double]$State.tray_rect.x + ([double]$State.tray_rect.width / 2.0))
    $centerY = [int][Math]::Round([double]$State.tray_rect.y + ([double]$State.tray_rect.height / 2.0))
    $hwnd = [IntPtr]::new([int64]("0x{0}" -f $State.tray_hwnd))

    [GuardianTrayNative]::SetCursorPos($centerX, $centerY) | Out-Null
    [void][GuardianTrayNative]::PostMessageW(
        $hwnd,
        [GuardianTrayNative]::WM_USER_TRAYICON,
        [UIntPtr]::Zero,
        [IntPtr][GuardianTrayNative]::WM_RBUTTONDOWN
    )
    Start-Sleep -Milliseconds 300
}

function Find-PopupMenuWindow {
    $script:__guardian_popup_menu_window = [IntPtr]::Zero
    $callback = [GuardianTrayNative+EnumWindowsProc]{
        param([IntPtr]$Hwnd, [IntPtr]$LParam)

        if (-not [GuardianTrayNative]::IsWindowVisible($Hwnd)) {
            return $true
        }

        $className = New-Object System.Text.StringBuilder 256
        [void][GuardianTrayNative]::GetClassNameW($Hwnd, $className, $className.Capacity)
        if ($className.ToString() -eq "#32768") {
            $script:__guardian_popup_menu_window = $Hwnd
            return $false
        }

        return $true
    }

    [void][GuardianTrayNative]::EnumWindows($callback, [IntPtr]::Zero)
    return $script:__guardian_popup_menu_window
}

function Wait-ForPopupMenuWindow {
    param(
        [int]$TimeoutSeconds = 3
    )

    $script:__guardian_waited_popup_menu_window = [IntPtr]::Zero
    Wait-Until -Description "popup menu window" -TimeoutSeconds $TimeoutSeconds -Condition {
        $menuWindow = Find-PopupMenuWindow
        if ($menuWindow -eq [IntPtr]::Zero) {
            return $false
        }

        $script:__guardian_waited_popup_menu_window = $menuWindow
        return $true
    } | Out-Null

    return $script:__guardian_waited_popup_menu_window
}

function Close-PopupMenu {
    param([IntPtr]$MenuWindow)

    if ($MenuWindow -ne [IntPtr]::Zero) {
        [void][GuardianTrayNative]::PostMessageW(
            $MenuWindow,
            [GuardianTrayNative]::WM_KEYDOWN,
            [UIntPtr]::new([uint64][GuardianTrayNative]::VK_ESCAPE),
            [IntPtr]::Zero
        )
        [void][GuardianTrayNative]::PostMessageW(
            $MenuWindow,
            [GuardianTrayNative]::WM_KEYUP,
            [UIntPtr]::new([uint64][GuardianTrayNative]::VK_ESCAPE),
            [IntPtr]::Zero
        )
        [void][GuardianTrayNative]::PostMessageW(
            $MenuWindow,
            [GuardianTrayNative]::WM_CANCELMODE,
            [UIntPtr]::Zero,
            [IntPtr]::Zero
        )
    }

    [System.Windows.Forms.SendKeys]::SendWait("{ESC}")
    Start-Sleep -Milliseconds 250
}

function Get-PopupMenuHandle {
    param([IntPtr]$MenuWindow)

    $menuHandle = [GuardianTrayNative]::SendMessageW(
        $MenuWindow,
        [GuardianTrayNative]::MN_GETHMENU,
        [UIntPtr]::Zero,
        [IntPtr]::Zero
    )

    if ($menuHandle -eq [IntPtr]::Zero) {
        throw "popup menu window did not return an HMENU handle."
    }

    return $menuHandle
}

function Find-PopupMenuItemRecursive {
    param(
        [IntPtr]$MenuHandle,
        [string]$Label
    )

    $menuItemCount = [GuardianTrayNative]::GetMenuItemCount($MenuHandle)
    if ($menuItemCount -lt 0) {
        throw "failed to query popup menu item count."
    }

    for ($index = 0; $index -lt $menuItemCount; $index++) {
        $text = New-Object System.Text.StringBuilder 512
        [void][GuardianTrayNative]::GetMenuStringW($MenuHandle, [uint32]$index, $text, $text.Capacity, 0x400)
        $textValue = $text.ToString()
        if ($textValue -eq $Label) {
            $itemId = [GuardianTrayNative]::GetMenuItemID($MenuHandle, $index)
            $stateBits = [GuardianTrayNative]::GetMenuState($MenuHandle, [uint32]$index, 0x400)
            return [pscustomobject]@{
                Id = [uint32]$itemId
                Index = $index
                Label = $textValue
                State = [uint32]$stateBits
                Enabled = (($stateBits -band 0x3) -eq 0)
            }
        }

        $subMenuHandle = [GuardianTrayNative]::GetSubMenu($MenuHandle, $index)
        if ($subMenuHandle -eq [IntPtr]::Zero) {
            continue
        }

        $nestedMatch = Find-PopupMenuItemRecursive -MenuHandle $subMenuHandle -Label $Label
        if ($null -ne $nestedMatch) {
            return $nestedMatch
        }
    }

    return $null
}

function Find-PopupMenuItem {
    param(
        [IntPtr]$MenuHandle,
        [string]$Label
    )

    $match = Find-PopupMenuItemRecursive -MenuHandle $MenuHandle -Label $Label
    if ($null -ne $match) {
        return $match
    }

    throw "popup menu item '$Label' was not found."
}

function Open-TrayPopupMenu {
    param(
        [object]$State,
        [switch]$TryShellClick
    )

    if ($TryShellClick) {
        try {
            Open-TrayMenuWithShellClick -State $State
            $menuWindow = Wait-ForPopupMenuWindow -TimeoutSeconds 1
            return [pscustomobject]@{
                MenuWindow = $menuWindow
                Origin = "shell-click"
            }
        } catch {
            Write-Step "shell click path did not expose a popup menu; falling back to tray window message"
            Close-OpenTrayMenu
        }
    }

    Open-TrayMenuWithWindowMessage -State $State
    $menuWindow = Wait-ForPopupMenuWindow -TimeoutSeconds 3
    return [pscustomobject]@{
        MenuWindow = $menuWindow
        Origin = "tray-window-message"
    }
}

function Try-OpenTrayPopupMenuWithShellClick {
    param([object]$State)

    try {
        Open-TrayMenuWithShellClick -State $State
        return Wait-ForPopupMenuWindow -TimeoutSeconds 1
    } catch {
        Close-OpenTrayMenu
        return [IntPtr]::Zero
    }
}

function Invoke-TrayPopupMenuItem {
    param(
        [object]$State,
        [string]$Label
    )

    $popupMenu = Open-TrayPopupMenu -State $State
    $menuHandle = Get-PopupMenuHandle -MenuWindow $popupMenu.MenuWindow
    $menuItem = Find-PopupMenuItem -MenuHandle $menuHandle -Label $Label
    if (-not $menuItem.Enabled) {
        throw "popup menu item '$Label' is disabled."
    }

    $ownerWindow = [IntPtr]::new([int64]("0x{0}" -f $State.tray_hwnd))
    [void][GuardianTrayNative]::PostMessageW(
        $ownerWindow,
        [GuardianTrayNative]::WM_COMMAND,
        [UIntPtr]::new([uint64]$menuItem.Id),
        [IntPtr]::Zero
    )
    Start-Sleep -Milliseconds 150
    Close-PopupMenu -MenuWindow $popupMenu.MenuWindow

    return [pscustomobject]@{
        Label = $menuItem.Label
        Id = $menuItem.Id
        Origin = $popupMenu.Origin
    }
}

function Wait-ForTrayState {
    param(
        [string]$StatePath,
        [int]$TimeoutSeconds,
        [scriptblock]$Predicate
    )

    $script:__guardian_tray_state = $null
    Wait-Until -Description "tray state predicate" -TimeoutSeconds $TimeoutSeconds -Condition {
        $state = Read-TestState -Path $StatePath
        if ($null -eq $state) {
            return $false
        }

        if (& $Predicate $state) {
            $script:__guardian_tray_state = $state
            return $true
        }

        return $false
    } | Out-Null

    return $script:__guardian_tray_state
}

function Assert-ZipContents {
    param([string]$ArchivePath)

    $zip = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
    try {
        $entryNames = @($zip.Entries | ForEach-Object { $_.FullName })
    } finally {
        $zip.Dispose()
    }

    $requiredEntries = @(
        "health-report.json",
        "profile-diagnosis.json",
        "audit-summary.json",
        "bundle-manifest.json"
    )

    foreach ($entry in $requiredEntries) {
        if ($entryNames -notcontains $entry) {
            throw "Archive '$ArchivePath' is missing required entry '$entry'."
        }
    }
}

function Send-TestCommand {
    param(
        [string]$CommandPath,
        [string]$Command
    )

    Set-Content -LiteralPath $CommandPath -Value $Command -NoNewline
}

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\..\.."))
if (-not $GuardianExe) {
    $GuardianExe = Join-Path $repoRoot "target\debug\guardian.exe"
}

if (-not (Test-Path -LiteralPath $GuardianExe)) {
    throw "guardian executable not found: $GuardianExe"
}

$smokeDir = Join-Path $repoRoot "target\tray-shell-smoke"
$statePath = Join-Path $smokeDir "tray-state.json"
$commandPath = Join-Path $smokeDir "tray-command.txt"
$stdoutPath = Join-Path $smokeDir "guardian-tray.stdout.log"
$stderrPath = Join-Path $smokeDir "guardian-tray.stderr.log"

New-Item -ItemType Directory -Force -Path $smokeDir | Out-Null
Remove-Item -LiteralPath $statePath,$commandPath,$stdoutPath,$stderrPath -ErrorAction SilentlyContinue

$previousTestStatePath = $env:GUARDIAN_TRAY_TEST_STATE_PATH
$previousCommandPath = $env:GUARDIAN_TRAY_TEST_COMMAND_PATH
$process = $null

try {
    Write-Step "starting guardian tray"
    $env:GUARDIAN_TRAY_TEST_STATE_PATH = $statePath
    $env:GUARDIAN_TRAY_TEST_COMMAND_PATH = $commandPath
    $process = Start-Process `
        -FilePath $GuardianExe `
        -ArgumentList @("tray") `
        -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath `
        -PassThru `
        -WindowStyle Hidden

    if ($null -eq $previousTestStatePath) { Remove-Item Env:GUARDIAN_TRAY_TEST_STATE_PATH -ErrorAction SilentlyContinue } else { $env:GUARDIAN_TRAY_TEST_STATE_PATH = $previousTestStatePath }
    if ($null -eq $previousCommandPath) { Remove-Item Env:GUARDIAN_TRAY_TEST_COMMAND_PATH -ErrorAction SilentlyContinue } else { $env:GUARDIAN_TRAY_TEST_COMMAND_PATH = $previousCommandPath }

    $readyState = Wait-ForTrayState -StatePath $statePath -TimeoutSeconds $StartupTimeoutSeconds -Predicate {
        param($state)
        return $state.tray_rect -and $state.tray_hwnd -and -not $state.action_in_flight
    }
    Write-Step "tray state file is ready"

    $startupCheckState = Wait-ForTrayState -StatePath $statePath -TimeoutSeconds $StartupTimeoutSeconds -Predicate {
        param($state)
        return -not $state.action_in_flight -and $state.last_action_text -like "最近动作：刷新整机检查 -> EXIT=*"
    }
    Write-Step ("startup check settled: {0}" -f $startupCheckState.last_action_text)

    if ([bool]$startupCheckState.open_latest_bundle_zip_enabled) {
        throw "Open Latest Bundle Zip should be disabled before any zip export."
    }
    Write-Step "confirmed latest zip action is disabled before export"

    Write-Step "probing shell tray click path"
    $shellProbeMenu = Try-OpenTrayPopupMenuWithShellClick -State $startupCheckState
    if ($shellProbeMenu -eq [IntPtr]::Zero) {
        Write-Step "shell click path did not expose a popup menu on this machine"
    } else {
        Write-Step "shell click path exposed a popup menu"
        Close-PopupMenu -MenuWindow $shellProbeMenu
    }

    Write-Step "triggering 导出诊断包并压缩 through the tray popup menu"
    $exportTrigger = $null
    try {
        $exportTrigger = Invoke-TrayPopupMenuItem -State $startupCheckState -Label "导出诊断包并压缩"
        Write-Step ("popup menu selected '{0}' via {1} (command id {2})" -f $exportTrigger.Label, $exportTrigger.Origin, $exportTrigger.Id)
    } catch {
        Write-Step ("popup menu automation for export failed; falling back to tray command channel. {0}" -f $_.Exception.Message)
        Send-TestCommand -CommandPath $commandPath -Command "ExportBundleZip"
    }

    $exportState = Wait-ForTrayState -StatePath $statePath -TimeoutSeconds $ActionTimeoutSeconds -Predicate {
        param($state)
        return (
            -not $state.action_in_flight -and
            $state.latest_bundle_archive -and
            $state.last_action_text -like "最近动作：导出诊断包并压缩 -> EXIT=*"
        )
    }

    $archivePath = [string]$exportState.latest_bundle_archive
    if (-not (Test-Path -LiteralPath $archivePath)) {
        throw "tray export reported archive '$archivePath' but the file does not exist."
    }

    Assert-ZipContents -ArchivePath $archivePath
    Write-Step ("tray export produced archive: {0}" -f $archivePath)

    if (-not [bool]$exportState.open_latest_bundle_zip_enabled) {
        throw "Open Latest Bundle Zip should be enabled after a zip export succeeds."
    }
    Write-Step "confirmed latest zip action is enabled after export"

    Write-Step "closing Guardian Tray"
    try {
        $exitTrigger = Invoke-TrayPopupMenuItem -State $exportState -Label "退出控制台托盘"
        Write-Step ("popup menu selected '{0}' via {1} (command id {2})" -f $exitTrigger.Label, $exitTrigger.Origin, $exitTrigger.Id)
    } catch {
        Write-Step ("popup menu automation for exit failed; falling back to tray command channel. {0}" -f $_.Exception.Message)
        Send-TestCommand -CommandPath $commandPath -Command "Exit"
    }

    try {
        Wait-ForTrayState -StatePath $statePath -TimeoutSeconds 5 -Predicate {
            param($state)
            return $state.last_action_text -eq "最近动作：托盘已请求退出"
        } | Out-Null
        Wait-ForGuardianExit -ProcessId $process.Id -TimeoutSeconds 15
    } catch {
        Write-Step "popup exit path did not stop the tray quickly enough; falling back to tray command channel"
        Send-TestCommand -CommandPath $commandPath -Command "Exit"
    }

    Wait-ForTrayState -StatePath $statePath -TimeoutSeconds 5 -Predicate {
        param($state)
        return $state.last_action_text -eq "最近动作：托盘已请求退出"
    } | Out-Null
    Wait-ForGuardianExit -ProcessId $process.Id -TimeoutSeconds 30

    Write-Step "tray shell smoke passed"
    Write-Host ("STATE={0}" -f $statePath)
    Write-Host ("ARCHIVE={0}" -f $archivePath)
    exit 0
}
finally {
    if ($null -eq $previousTestStatePath) { Remove-Item Env:GUARDIAN_TRAY_TEST_STATE_PATH -ErrorAction SilentlyContinue } else { $env:GUARDIAN_TRAY_TEST_STATE_PATH = $previousTestStatePath }
    if ($null -eq $previousCommandPath) { Remove-Item Env:GUARDIAN_TRAY_TEST_COMMAND_PATH -ErrorAction SilentlyContinue } else { $env:GUARDIAN_TRAY_TEST_COMMAND_PATH = $previousCommandPath }

    if ($process -and (Get-Process -Id $process.Id -ErrorAction SilentlyContinue)) {
        Stop-Process -Id $process.Id -Force
    }
}
