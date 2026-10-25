[CmdletBinding()]
param([string]$InstallRoot, [string]$PreviousDirectory, [string]$Directory)
$ErrorActionPreference = 'Stop'

function Get-RetypePayloadDirectory([string]$Root, [string]$Path) {
  $rootPath = [IO.Path]::GetFullPath($Root).TrimEnd('\')
  $payload = [IO.Path]::GetFullPath($Path).TrimEnd('\')
  $parent = [IO.Path]::GetDirectoryName($payload)
  if ($payload -ine $rootPath -and $parent -ine (Join-Path $rootPath 'versions')) {
    throw 'The sync program must belong to this retype installation.'
  }
  return $payload
}

function Move-RetypeFirewallRules([string]$Root, [string]$Previous, [string]$Current) {
  $target = Join-Path (Get-RetypePayloadDirectory $Root $Current) 'retype-sync-host.exe'
  if (-not (Test-Path -LiteralPath $target -PathType Leaf)) { throw 'The new sync program is missing.' }
  if ([string]::IsNullOrWhiteSpace($Previous)) { return 0 }
  $source = Join-Path (Get-RetypePayloadDirectory $Root $Previous) 'retype-sync-host.exe'
  if ($source -ieq $target) { return 0 }
  $count = 0
  # Only local persistent rules for the immediately previous executable qualify.
  # Change the application filter in place, retaining allow/block, profiles,
  # enabled state, ports, addresses, interfaces, and any other rule restrictions.
  $filters = @(Get-NetFirewallApplicationFilter -PolicyStore PersistentStore | Where-Object {
    $program = [Environment]::ExpandEnvironmentVariables([string]$_.Program).Trim('"')
    $program -ieq $source
  })
  foreach ($filter in $filters) {
    Set-NetFirewallApplicationFilter -InputObject $filter -Program $target -ErrorAction Stop | Out-Null
    $count++
  }
  return $count
}

if ($MyInvocation.InvocationName -ne '.') {
  try {
    $count = Move-RetypeFirewallRules $InstallRoot $PreviousDirectory $Directory
    Write-Output "Preserved $count existing sync firewall rules."
  } catch {
    Write-Error $_
    exit 1
  }
}
