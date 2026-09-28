# Destructive to retype's installation; only run on an ephemeral, clean CI runner.
[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$Setup)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Requires an ephemeral Windows Actions runner.' }
$root = Join-Path $env:ProgramFiles 'retype'
if (Test-Path -LiteralPath $root) { throw 'Requires a clean runner without retype.' }
function Install-Package($File, $Log) {
  $p = Start-Process -FilePath $File -ArgumentList @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/NOCLOSEAPPLICATIONS','/RESTARTEXITCODE=3010',('/LOG="'+$Log+'"')) -WindowStyle Hidden -Wait -PassThru
  if ($p.ExitCode -ne 0) { Get-Content -LiteralPath $Log -Tail 80; throw "Installation failed or needed reboot: $($p.ExitCode)" }
}
# Exercise migration from the actually published flat-directory installer.
$legacy = Join-Path $env:RUNNER_TEMP 'legacy-retype'
New-Item -ItemType Directory -Path $legacy | Out-Null
gh release download v0.1.4 --repo L-Chris/retype --pattern '*setup.exe*' --dir $legacy
if ($LASTEXITCODE -ne 0) { throw 'Could not fetch legacy release.' }
$legacySetup = Join-Path $legacy 'retype-0.1.4-windows-x64-setup.exe'
$expected = ((Get-Content "$legacySetup.sha256" -Raw).Trim() -split '\s+')[0]
if ((Get-FileHash $legacySetup).Hash -ne $expected) { throw 'Legacy installer checksum mismatch.' }
Install-Package $legacySetup (Join-Path $env:RUNNER_TEMP 'legacy-install.log')
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class LoadedTip {
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  public static extern IntPtr LoadLibrary(string file);
  [DllImport("kernel32.dll")]
  public static extern bool FreeLibrary(IntPtr module);
}
'@
$oldDll = Join-Path $root 'retype_ime.dll'
$oldHash = (Get-FileHash $oldDll).Hash
$held = [LoadedTip]::LoadLibrary($oldDll)
if ($held -eq [IntPtr]::Zero) { throw 'Could not hold legacy DLL in memory.' }
$heldVersion = [IntPtr]::Zero
try {
  Install-Package (Resolve-Path $Setup) (Join-Path $env:RUNNER_TEMP 'versioned-upgrade.log')
  . "$PSScriptRoot\update-common.ps1"
  $first = Get-RetypeInstallation
  Test-RetypeInstallation $first
  if ($first.Directory -notlike "$root\versions\*") { throw 'Payload was not installed in a version directory.' }
  if ((Get-FileHash $oldDll).Hash -ne $oldHash) { throw 'Upgrade overwrote the loaded legacy DLL.' }
  $firstDll = Join-Path $first.Directory 'retype_ime.dll'
  $heldVersion = [LoadedTip]::LoadLibrary($firstDll)
  if ($heldVersion -eq [IntPtr]::Zero) { throw 'Could not hold first versioned DLL.' }
  # Same-version repair must also allocate a fresh directory.
  Install-Package (Resolve-Path $Setup) (Join-Path $env:RUNNER_TEMP 'same-version-repair.log')
  $second = Get-RetypeInstallation
  if ($second.Directory -eq $first.Directory) { throw 'Repair reused a loaded payload directory.' }
  Test-RetypeInstallation $second
  $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
  if (-not (Get-ScheduledTask -TaskName "retype-update-$sid")) { throw 'Automatic update task is missing.' }
  cargo test --release --target x86_64-pc-windows-msvc -p retype-tsf installed_tip_ -- --ignored --test-threads=1
  if ($LASTEXITCODE -ne 0) { throw 'Installed TIP activation verification failed.' }
  $background = Start-Process powershell -ArgumentList ('-NoProfile -STA -WindowStyle Hidden -ExecutionPolicy Bypass -File "'+$second.Directory+'\update-ui.ps1" -Background') -WindowStyle Hidden -PassThru
  if (-not $background.WaitForExit(90000)) { $background.Kill(); throw 'Background update check did not exit.' }
  $state = Get-Content "$env:LOCALAPPDATA\retype\updates\state.json" -Raw | ConvertFrom-Json
  if ($state.Stage -ne 'up_to_date') { throw "Background check failed: $($state.Error)" }
  Write-Output 'Legacy migration, occupied-DLL repair, registration and background update check passed without reboot.'
} finally {
  if ($heldVersion -ne [IntPtr]::Zero) { [LoadedTip]::FreeLibrary($heldVersion) | Out-Null }
  [LoadedTip]::FreeLibrary($held) | Out-Null
}
