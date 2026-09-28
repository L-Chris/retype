$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\update-common.ps1"
$testDirectory = Join-Path ([IO.Path]::GetTempPath()) ('retype-state-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testDirectory | Out-Null
$path = Join-Path $testDirectory 'state.json'
try {
  $state = Read-UpdateState $path
  if (-not (Test-UpdateDue $state)) { throw 'New installation must allow check.' }
  $state.LastCheck = [DateTime]::UtcNow.ToString('o')
  if (Test-UpdateDue $state) { throw 'Repeated check must be throttled.' }
  $state.LastCheck = [DateTime]::UtcNow.AddHours(-25).ToString('o')
  if (-not (Test-UpdateDue $state)) { throw 'Daily check did not become due.' }
  $state.AutoCheck = $false
  if (Test-UpdateDue $state) { throw 'Opt-out must suppress background checks.' }
  $state.Stage = 'installing'; $state.TargetVersion = '0.1.5'
  Save-UpdateState $state $path
  $loaded = Read-UpdateState $path
  if ($loaded.TargetVersion -ne '0.1.5' -or $loaded.AutoCheck -or $loaded.Stage -ne 'installing') { throw 'Interrupted installation state was lost.' }
  $state.Stage = 'complete'; Save-UpdateState $state $path
  if ((Read-UpdateState $path).Stage -ne 'complete') { throw 'Atomic state replacement failed.' }
  [IO.File]::WriteAllText($path,'invalid JSON')
  if (-not (Read-UpdateState $path).AutoCheck) { throw 'Corrupt state prevented recovery.' }
  Write-Output 'Update state tests passed.'
} finally {
  # Both files are owned by this test; do not recursively remove a computed directory.
  Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath "$path.tmp" -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $testDirectory -ErrorAction SilentlyContinue
}
