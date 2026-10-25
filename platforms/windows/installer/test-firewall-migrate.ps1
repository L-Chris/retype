# Changes only temporary firewall fixtures; allow rules stay disabled.
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Requires an ephemeral Windows Actions runner.' }
. "$PSScriptRoot\firewall-migrate.ps1"
$id = 'retype-firewall-test-' + [Guid]::NewGuid().ToString('N')
$root = Join-Path $env:RUNNER_TEMP $id
$old = Join-Path $root 'versions\old'
$new = Join-Path $root 'versions\new'
$other = Join-Path $root 'versions\unrelated'
$names = @("$id-allow", "$id-block", "$id-other", "$id-stale")
function Read-TestRule($Name) {
  $rule = Get-NetFirewallRule -Name $Name -PolicyStore PersistentStore
  $port = $rule | Get-NetFirewallPortFilter
  $address = $rule | Get-NetFirewallAddressFilter
  $application = $rule | Get-NetFirewallApplicationFilter
  return @{
    Program = $application.Program; Action = [string]$rule.Action
    Enabled = [string]$rule.Enabled; Profile = [string]$rule.Profile
    Direction = [string]$rule.Direction; Protocol = [string]$port.Protocol
    LocalPort = @($port.LocalPort) -join ','; RemoteAddress = @($address.RemoteAddress) -join ','
  }
}
try {
  foreach ($dir in @($old, $new, $other)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $dir 'retype-sync-host.exe'), 'inert firewall test fixture')
  }
  New-NetFirewallRule -Name $names[0] -DisplayName $names[0] -PolicyStore PersistentStore -Program (Join-Path $old 'retype-sync-host.exe') -Action Allow -Profile Private -Protocol TCP -LocalPort 50206 -RemoteAddress 192.168.0.0/16 -Direction Inbound -Enabled False | Out-Null
  New-NetFirewallRule -Name $names[1] -DisplayName $names[1] -PolicyStore PersistentStore -Program (Join-Path $old 'retype-sync-host.exe') -Action Block -Profile Public -Protocol UDP -LocalPort 5353 -RemoteAddress 100.64.0.0/10 -Direction Inbound -Enabled True | Out-Null
  New-NetFirewallRule -Name $names[2] -DisplayName $names[2] -PolicyStore PersistentStore -Program (Join-Path $old 'other.exe') -Action Allow -Enabled False | Out-Null
  New-NetFirewallRule -Name $names[3] -DisplayName $names[3] -PolicyStore PersistentStore -Program (Join-Path $other 'retype-sync-host.exe') -Action Allow -Enabled False | Out-Null
  $before = @{}; foreach ($name in $names) { $before[$name] = Read-TestRule $name }
  if ((Move-RetypeFirewallRules $root '' $new) -ne 0) { throw 'A new installation must not create authorization.' }
  if ((Move-RetypeFirewallRules $root $old $new) -ne 2) { throw 'Only the previous sync executable rules may migrate.' }
  foreach ($name in $names) {
    $after = Read-TestRule $name
    foreach ($key in $before[$name].Keys) {
      $expected = $before[$name][$key]
      if ($key -eq 'Program' -and $name -in $names[0..1]) { $expected = Join-Path $new 'retype-sync-host.exe' }
      if ($after[$key] -ine $expected) { throw "Firewall rule $name changed $key unexpectedly." }
    }
  }
  if ((Move-RetypeFirewallRules $root $old $new) -ne 0) { throw 'Repeated migration must not duplicate rules.' }
  if ((Move-RetypeFirewallRules $root $new $old) -ne 2) { throw 'A rollback must restore the previous paths.' }
  $rejected = $false
  try { Move-RetypeFirewallRules $root $old (Join-Path $env:RUNNER_TEMP 'unrelated') | Out-Null } catch { $rejected = $true }
  if (-not $rejected) { throw 'A target outside this installation must be rejected.' }
  Write-Output 'Firewall migration preserves allow/block, profiles, enabled state, addresses and ports; unrelated rules and first-install consent remain unchanged.'
} finally {
  foreach ($name in $names) { Remove-NetFirewallRule -Name $name -PolicyStore PersistentStore -ErrorAction SilentlyContinue }
  Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
