param([string]$Executable, [string]$OutputDirectory)
$ErrorActionPreference = 'Stop'
$taskRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Executable) { $Executable = Join-Path $taskRoot 'target\release\retype-settings-egui.exe' }
$Executable = (Resolve-Path -LiteralPath $Executable).Path
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $taskRoot 'target\settings-egui-smoke' }
[void](New-Item -ItemType Directory -Force -Path $OutputDirectory)
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path

if (-not ('RetypeSettingsSmokeWindow' -as [type])) {
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class RetypeSettingsSmokeWindow {
  [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left,Top,Right,Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window,out Rect rect);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr window,out Rect rect);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr window,uint message,IntPtr wp,IntPtr lp);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
  private delegate bool EnumWindowCallback(IntPtr window, IntPtr parameter);
  [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindowCallback callback, IntPtr parameter);
  [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
  public static IntPtr MainWindow(int processId) {
    IntPtr result = IntPtr.Zero;
    EnumWindows((window, parameter) => {
      uint pid; GetWindowThreadProcessId(window, out pid);
      Rect rect;
      if (pid == processId && IsWindowVisible(window) && GetClientRect(window, out rect) && rect.Right > 100) { result = window; return false; }
      return true;
    }, IntPtr.Zero);
    return result;
  }
}
'@
}
function Start-Settings([string]$Path, [string[]]$Arguments) {
  Start-Process -FilePath $Path -ArgumentList $Arguments -WindowStyle Hidden -PassThru
}
function Wait-SettingsWindow($Process) {
  $deadline = [DateTime]::UtcNow.AddSeconds(20)
  do {
    $Process.Refresh()
    if ($Process.HasExited) { throw 'Settings exited before showing a window.' }
    $mainWindow = [RetypeSettingsSmokeWindow]::MainWindow($Process.Id)
    if ($mainWindow -ne [IntPtr]::Zero) { return $mainWindow }
    Start-Sleep -Milliseconds 25
  } while ([DateTime]::UtcNow -lt $deadline)
  throw 'Settings window did not appear.'
}
function Wait-Visibility([IntPtr]$Window, [bool]$Visible) {
  $deadline = [DateTime]::UtcNow.AddSeconds(5)
  while ([RetypeSettingsSmokeWindow]::IsWindowVisible($Window) -ne $Visible) {
    if ([DateTime]::UtcNow -gt $deadline) { throw "Expected visibility: $Visible" }
    Start-Sleep -Milliseconds 25
  }
}
foreach ($page in @('input','dictionary','statistics','about')) {
  $screenshot = Join-Path $OutputDirectory "$page.png"
  $process = Start-Settings $Executable @("--page=$page", "--screenshot=`"$screenshot`"", '--exit-after-ms=1500')
  try {
    if (-not $process.WaitForExit(20000)) { throw "$page did not exit" }
    if ($process.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $screenshot)) { throw "$page smoke test failed" }
  } finally { if (-not $process.HasExited) { Stop-Process -Id $process.Id } }
}

$report = Join-Path $OutputDirectory 'warm.json'
$process = Start-Settings $Executable @("--timing-file=`"$report`"", '--hide-after-ms=800')
$other = $null
try {
  $window = Wait-SettingsWindow $process
  Start-Sleep -Milliseconds 500
  Wait-Visibility $window $false
  if ($process.HasExited) { throw 'Custom close terminated the retained process' }
  # --updates must be forwarded to the retained About page, not a second UI process.
  $duplicate = Start-Settings $Executable @('--updates')
  if (-not $duplicate.WaitForExit(10000) -or $duplicate.ExitCode -ne 0) { throw 'Same-version forwarding failed' }
  Wait-Visibility $window $true
  if ($process.HasExited) { throw 'Warm activation replaced the original process' }
  $deadline = [DateTime]::UtcNow.AddSeconds(5)
  do {
    $activePage = (Get-Content -LiteralPath $report -Raw | ConvertFrom-Json).page
    if ($activePage -eq 'About') { break }
    Start-Sleep -Milliseconds 25
  } while ([DateTime]::UtcNow -lt $deadline)
  if ($activePage -ne 'About') { throw 'Update request did not select the About page' }

  $otherDirectory = Join-Path $OutputDirectory 'other-install\settings'
  [void](New-Item -ItemType Directory -Force -Path $otherDirectory)
  $otherExecutable = Join-Path $otherDirectory 'retype.exe'
  Copy-Item -LiteralPath $Executable -Destination $otherExecutable -Force
  $other = Start-Settings $otherExecutable @('--page=about')
  $otherWindow = Wait-SettingsWindow $other
  if ($other.Id -eq $process.Id) { throw 'Different install paths shared an instance' }
  [void][RetypeSettingsSmokeWindow]::PostMessageW($otherWindow,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)
  if (-not $other.WaitForExit(5000)) { throw 'Native close must terminate the process for upgrades' }
  [void][RetypeSettingsSmokeWindow]::PostMessageW($window,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)
  if (-not $process.WaitForExit(5000)) { throw 'Native close did not terminate settings' }
  $process = Start-Settings $Executable @('--idle-exit-ms=1000', '--hide-after-ms=800')
  $window = Wait-SettingsWindow $process
  Start-Sleep -Milliseconds 500
  Wait-Visibility $window $false
  if (-not $process.WaitForExit(5000) -or $process.ExitCode -ne 0) { throw 'Hidden idle timer did not exit cleanly' }
  Write-Output 'Settings smoke tests passed: four pages, hide/restore, warm activation, update routing, version isolation, native close, idle expiry.'
} finally {
  foreach ($owned in @($process,$other)) { if ($owned -and -not $owned.HasExited) { Stop-Process -Id $owned.Id } }
}
