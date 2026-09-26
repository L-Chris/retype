<#
.SYNOPSIS
  Add/remove retype in the current user's Windows keyboard list (no elevation needed).
.DESCRIPTION
  Machine registration alone does not add a TIP to the Windows user language list.
  Call InstallLayoutOrTip without default/clean-install flags, preserving other keyboards.
  Run as the actual desktop user, not an alternate administrator account.
#>
[CmdletBinding()]
param([switch]$Uninstall)
$ErrorActionPreference = 'Stop'
$tip = '0804:{7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52}{A3F1C6D9-2E47-4B8A-9C51-6D0E8F2A3B74}'

function Get-UserTips {
  @(Get-WinUserLanguageList | ForEach-Object { $_.InputMethodTips })
}

$before = @(Get-UserTips)
if ($Uninstall -and $before -notcontains $tip) { return }
if (-not $Uninstall) {
  $profile = 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\CTF\TIP\{7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52}\LanguageProfile\0x00000804\{A3F1C6D9-2E47-4B8A-9C51-6D0E8F2A3B74}'
  if (-not (Test-Path -LiteralPath $profile)) { throw 'Install retype before adding its keyboard.' }
}
if (-not ('Retype.UserInputProfile' -as [type])) {
  Add-Type -TypeDefinition @'
using System.Runtime.InteropServices;
namespace Retype {
    public static class UserInputProfile {
        [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
        [DllImport("input.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool InstallLayoutOrTip(string profile, uint flags);
    }
}
'@
}
# 0 = append/enable, 1 = uninstall. Never change the user's default input method.
$flags = if ($Uninstall) { [uint32]1 } else { [uint32]0 }
if (-not [Retype.UserInputProfile]::InstallLayoutOrTip($tip, $flags)) {
  throw 'InstallLayoutOrTip failed. The keyboard was not successfully configured.'
}
# Read the user language list, not only TSF's Enable registry value.
for ($attempt = 0; $attempt -lt 10; $attempt++) {
  $after = @(Get-UserTips)
  $present = $after -contains $tip
  if ($present -ne [bool]$Uninstall) { break }
  Start-Sleep -Milliseconds 300
}
if ($present -eq [bool]$Uninstall) { throw 'Windows has not reflected the requested keyboard change in the user language list.' }
$missing = @($before | Where-Object { $_ -ne $tip -and $after -notcontains $_ })
if ($missing.Count) { throw "Other keyboard entries changed during registration: $($missing -join ', ')" }
Write-Host $(if ($Uninstall) { 'retype removed from the current user keyboard list.' } else { 'retype added to the current user keyboard list. Use Win+Space in a desktop app.' })
