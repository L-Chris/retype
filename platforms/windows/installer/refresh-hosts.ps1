[CmdletBinding()]
param([switch]$ReportOnly)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\update-common.ps1"

$installation = Get-RetypeInstallation
$active = @(
  (Join-Path $installation.Directory 'retype_ime.dll'),
  (Join-Path $installation.Directory 'x86\retype_ime.dll')
)
$stale = @()
$restartedSearch = $false
$restartSettings = $false
$settings = Join-Path $installation.Directory 'settings\retype.exe'
$versions = (Split-Path $installation.Directory -Parent).TrimEnd('\') + '\'

foreach ($process in Get-Process -ErrorAction SilentlyContinue) {
  try {
    $processPath = $process.Path
    $oldSettings = $process.ProcessName -eq 'retype' -and $processPath -and
      $processPath.StartsWith($versions, [StringComparison]::OrdinalIgnoreCase) -and
      -not [string]::Equals($processPath, $settings, [StringComparison]::OrdinalIgnoreCase)
    $loaded = @($process.Modules | Where-Object { $_.ModuleName -eq 'retype_ime.dll' })
    $old = @($loaded | Where-Object { $active -notcontains $_.FileName })
    if ($old.Count -eq 0 -and -not $oldSettings) { continue }

    # Windows Search is a disposable system host. Restart only its stale instance;
    # never terminate an editor or browser, which may contain unsaved work.
    if ($process.ProcessName -eq 'SearchHost' -and -not $ReportOnly) {
      Stop-Process -Id $process.Id -ErrorAction Stop
      $restartedSearch = $true
      continue
    }
    if ($oldSettings -and -not $ReportOnly -and
        $process.MainWindowHandle -ne [IntPtr]::Zero -and $process.CloseMainWindow() -and
        $process.WaitForExit(5000)) {
      $restartSettings = $true
      continue
    }
    $stale += [pscustomobject]@{
      Id = $process.Id
      Name = $process.ProcessName
      Title = $process.MainWindowTitle
    }
  } catch [System.ComponentModel.Win32Exception] {
    # Protected or elevated processes cannot be inspected from the user's session.
  } catch [System.InvalidOperationException] {
    # The process exited while we were enumerating it.
  }
}

if ($restartSettings -and (Test-Path -LiteralPath $settings)) {
  Start-Process -FilePath $settings -ArgumentList '--updates'
}

$report = [pscustomobject]@{
  ActiveVersion = $installation.Version
  SearchHostRestarted = $restartedSearch
  SettingsRestarted = $restartSettings
  StaleApplications = @($stale)
}
$path = Join-Path $env:LOCALAPPDATA 'retype\updates\refresh.json'
if (-not $ReportOnly) {
  New-Item -ItemType Directory -Path (Split-Path $path) -Force | Out-Null
  [IO.File]::WriteAllText($path, ($report | ConvertTo-Json -Depth 4), (New-Object Text.UTF8Encoding($false)))
}
$report | ConvertTo-Json -Depth 4
