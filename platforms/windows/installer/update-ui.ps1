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

# Scheduled checks remain in a background helper. Any user interaction happens
# in Settings > About, so there is only one update interface.
$cache = Join-Path $env:LOCALAPPDATA 'retype\updates'
New-Item -ItemType Directory -Path $cache -Force | Out-Null
$statePath = Join-Path $cache 'state.json'
$state = Read-UpdateState $statePath
if (-not (Test-UpdateDue $state)) { return }
$state.LastCheck = [DateTime]::UtcNow.ToString('o')
Save-UpdateState $state $statePath
try {
  $json = & (Join-Path $installation.Directory 'retype-updater.exe') check --repo L-Chris/retype --current $installation.Version --json
  $code = $LASTEXITCODE
  $offer = $json | ConvertFrom-Json
  if ($code -notin @(0, 10)) { throw $(if ($offer.error) { $offer.error } else { "更新器退出码：$code" }) }
  $state.Error = ''
  Save-UpdateState $state $statePath
  if ($offer.update_available -and $offer.latest.version -ne $state.SkippedVersion) {
    Start-Process -FilePath $settings -ArgumentList '--updates'
  }
} catch {
  $state.Error = $_.Exception.Message
  Save-UpdateState $state $statePath
}
