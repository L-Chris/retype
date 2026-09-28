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
using System;
using System.Runtime.InteropServices;
namespace Retype {
    [ComImport, Guid("1F02B6C5-7842-4EE6-8A0B-9A24183A95CA"),
     InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    internal interface IInputProcessorProfiles {
        void Register(ref Guid clsid);
        void Unregister(ref Guid clsid);
        void AddLanguageProfile(ref Guid clsid, ushort lang, ref Guid profile, IntPtr description, uint descriptionLength, IntPtr icon, uint iconLength, uint iconIndex);
        void RemoveLanguageProfile(ref Guid clsid, ushort lang, ref Guid profile);
        void EnumInputProcessorInfo(out IntPtr enumerator);
        void GetDefaultLanguageProfile(ushort lang, ref Guid category, out Guid clsid, out Guid profile);
        void SetDefaultLanguageProfile(ushort lang, ref Guid clsid, ref Guid profile);
        void ActivateLanguageProfile(ref Guid clsid, ushort lang, ref Guid profile);
        void GetActiveLanguageProfile(ref Guid clsid, out ushort lang, out Guid profile);
        void GetLanguageProfileDescription(ref Guid clsid, ushort lang, ref Guid profile, out IntPtr description);
        void GetCurrentLanguage(out ushort lang);
        void ChangeCurrentLanguage(ushort lang);
        void GetLanguageList(out IntPtr languages, out uint count);
        void EnumLanguageProfiles(ushort lang, out IntPtr enumerator);
        void EnableLanguageProfile(ref Guid clsid, ushort lang, ref Guid profile, [MarshalAs(UnmanagedType.Bool)] bool enable);
        void IsEnabledLanguageProfile(ref Guid clsid, ushort lang, ref Guid profile, [MarshalAs(UnmanagedType.Bool)] out bool enabled);
    }
    public static class UserInputProfile {
        public static void SetEnabled(bool enabled) {
            var instance = Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("33C53A50-F456-4884-B049-85FD643ECFED")));
            try {
                var profiles = (IInputProcessorProfiles)instance;
                var clsid = new Guid("7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52");
                var profile = new Guid("A3F1C6D9-2E47-4B8A-9C51-6D0E8F2A3B74");
                profiles.EnableLanguageProfile(ref clsid, 0x0804, ref profile, enabled);
                bool actual;
                profiles.IsEnabledLanguageProfile(ref clsid, 0x0804, ref profile, out actual);
                if (actual != enabled) throw new InvalidOperationException("TSF profile enablement did not persist.");
            } finally {
                Marshal.ReleaseComObject(instance);
            }
        }
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
# A keyboard-list entry can already exist while TSF still considers it disabled.
# Set and verify current-user enablement independently; do not change the default.
[Retype.UserInputProfile]::SetEnabled(-not [bool]$Uninstall)
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
