[CmdletBinding()]
param([switch]$Background)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\update-common.ps1"

$installation = Get-RetypeInstallation
$settings = Join-Path $installation.Directory 'settings\retype.exe'
if (-not (Test-Path -LiteralPath $settings)) { throw '找不到 retype 设置程序。' }

if (-not $Background) {
  Start-Process -FilePath $settings -ArgumentList '--updates'
  return
}

# Compatibility for old callers; newly registered tasks use the native GUI launcher.
Start-Process -FilePath (Join-Path $installation.Directory 'retype-updater.exe') -ArgumentList 'update --background' -WindowStyle Hidden
