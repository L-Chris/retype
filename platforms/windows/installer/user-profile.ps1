<#
.SYNOPSIS
  Add/remove retype in the current user's Windows keyboard list (no elevation needed).
.DESCRIPTION
  Machine registration alone does not add a TIP to the Windows user language list.
  Call InstallLayoutOrTip without default/clean-install flags, preserving other keyboards.
  Run as the actual desktop user, not an alternate administrator account.
#>
[CmdletBinding()]
param([switch]$Uninstall, [switch]$Verify)
$ErrorActionPreference = 'Stop'
$entries = @(
  @{ Language = [ushort]0x0804; Profile = 'A3F1C6D9-2E47-4B8A-9C51-6D0E8F2A3B74'; Name = 'retype Chinese' },
  @{ Language = [ushort]0x0409; Profile = '8E082735-45E3-4D99-8BCA-A5CD3591C4E6'; Name = 'retype English' }
)
foreach ($entry in $entries) {
  $entry.Tip = ('{0:X4}:{{7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52}}{{{1}}}' -f $entry.Language, $entry.Profile)
}
$tips = @($entries | ForEach-Object { $_.Tip })

function Get-UserTips {
  @(Get-WinUserLanguageList | ForEach-Object { $_.InputMethodTips })
}

$before = @(Get-UserTips)
if ($Uninstall) {
  $syncHost = Join-Path $PSScriptRoot 'retype-sync-host.exe'
  if (Test-Path -LiteralPath $syncHost) {
    Start-Process -FilePath $syncHost -ArgumentList '--stop' -WindowStyle Hidden -Wait | Out-Null
  }
  Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'retype Learning' -ErrorAction SilentlyContinue
  $learningHost = Join-Path $PSScriptRoot 'retype-learning-host.exe'
  if (Test-Path -LiteralPath $learningHost) {
    Start-Process -FilePath $learningHost -ArgumentList '--stop' -WindowStyle Hidden -Wait | Out-Null
  }
  if (-not @($before | Where-Object { $tips -contains $_ }).Count) { return }
}
if (-not $Uninstall) {
  foreach ($entry in $entries) {
    $languageKey = '0x{0:x8}' -f $entry.Language
    $profile = "Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\CTF\TIP\{7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52}\LanguageProfile\$languageKey\{$($entry.Profile)}"
    if (-not (Test-Path -LiteralPath $profile)) { throw "Install $($entry.Name) before adding its keyboard." }
  }
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
        public static bool IsEnabled(ushort language, string profileId) {
            var instance = Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("33C53A50-F456-4884-B049-85FD643ECFED")));
            try {
                var profiles = (IInputProcessorProfiles)instance;
                var clsid = new Guid("7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52");
                var profile = new Guid(profileId);
                bool enabled;
                profiles.IsEnabledLanguageProfile(ref clsid, language, ref profile, out enabled);
                return enabled;
            } finally { Marshal.ReleaseComObject(instance); }
        }
        public static void SetEnabled(ushort language, string profileId, bool enabled) {
            var instance = Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("33C53A50-F456-4884-B049-85FD643ECFED")));
            try {
                var profiles = (IInputProcessorProfiles)instance;
                var clsid = new Guid("7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52");
                var profile = new Guid(profileId);
                profiles.EnableLanguageProfile(ref clsid, language, ref profile, enabled);
                bool actual;
                profiles.IsEnabledLanguageProfile(ref clsid, language, ref profile, out actual);
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
if ($Verify) {
  foreach ($entry in $entries) {
    if ($before -notcontains $entry.Tip -or -not [Retype.UserInputProfile]::IsEnabled($entry.Language, $entry.Profile)) {
      throw "$($entry.Name) is missing or disabled for the current user."
    }
  }
  return
}
if (-not $Uninstall) {
  # TIPs in AppContainer hosts (including Windows Search) must read the same
  # input preferences as desktop apps. Share this settings key read-only;
  # preserve its existing ACL and do not grant access to sibling/subkeys.
  $preferences = 'HKCU:\Software\retype'
  if (-not (Test-Path $preferences)) { New-Item -Path $preferences -Force | Out-Null }
  $acl = Get-Acl -Path $preferences
  $packages = [Security.Principal.SecurityIdentifier]::new('S-1-15-2-1')
  $read = [Security.AccessControl.RegistryAccessRule]::new($packages,
    [Security.AccessControl.RegistryRights]::ReadKey,
    [Security.AccessControl.InheritanceFlags]::None,
    [Security.AccessControl.PropagationFlags]::None,
    [Security.AccessControl.AccessControlType]::Allow)
  $acl.SetAccessRule($read)
  Set-Acl -Path $preferences -AclObject $acl

  # Dedicated startup-only diagnostics, including failures in AppContainer hosts.
  # No typed text, credentials or provider configuration is written here.
  $settingsLogs = Join-Path $env:LOCALAPPDATA 'retype\logs'
  New-Item -ItemType Directory -Path $settingsLogs -Force | Out-Null
  $logsAcl = Get-Acl -LiteralPath $settingsLogs
  $logsWrite = [Security.AccessControl.FileSystemAccessRule]::new($packages,
    [Security.AccessControl.FileSystemRights]'Read, Write',
    [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit',
    [Security.AccessControl.PropagationFlags]::None,
    [Security.AccessControl.AccessControlType]::Allow)
  $logsAcl.SetAccessRule($logsWrite)
  Set-Acl -LiteralPath $settingsLogs -AclObject $logsAcl
  Set-ItemProperty -Path $preferences -Name 'SettingsLogPath' -Value $settingsLogs

  # The TIP runs inside desktop and AppContainer hosts. Give the dedicated
  # count-only directory (not the whole retype data tree) write access, and
  # publish its real user path since AppContainer LOCALAPPDATA is redirected.
  $statistics = Join-Path $env:LOCALAPPDATA 'retype\statistics'
  New-Item -ItemType Directory -Path $statistics -Force | Out-Null
  $statsAcl = Get-Acl -LiteralPath $statistics
  $statsWrite = [Security.AccessControl.FileSystemAccessRule]::new($packages,
    [Security.AccessControl.FileSystemRights]'Read, Write',
    [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit',
    [Security.AccessControl.PropagationFlags]::None,
    [Security.AccessControl.AccessControlType]::Allow)
  $statsAcl.SetAccessRule($statsWrite)
  Set-Acl -LiteralPath $statistics -AclObject $statsAcl
  Set-ItemProperty -Path $preferences -Name 'StatisticsPath' -Value $statistics

  # Category dictionaries live outside versioned installs. Search/AppContainer
  # hosts must read the real profile path, but never write downloaded files.
  $dictionaryRoot = Join-Path $env:LOCALAPPDATA 'retype\dict-packs'
  New-Item -ItemType Directory -Path $dictionaryRoot -Force | Out-Null
  $packAcl = Get-Acl -LiteralPath $dictionaryRoot
  $packRead = [Security.AccessControl.FileSystemAccessRule]::new($packages,
    [Security.AccessControl.FileSystemRights]::ReadAndExecute,
    [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit',
    [Security.AccessControl.PropagationFlags]::None,
    [Security.AccessControl.AccessControlType]::Allow)
  $packAcl.SetAccessRule($packRead)
  Set-Acl -LiteralPath $dictionaryRoot -AclObject $packAcl
  Set-ItemProperty -Path $preferences -Name 'DictionaryRoot' -Value $dictionaryRoot

  # Public base images share read access only; personal learning stays private.
  $baseRoot = Join-Path $env:LOCALAPPDATA 'retype\dictionaries'
  New-Item -ItemType Directory -Path $baseRoot -Force | Out-Null
  $baseAcl = Get-Acl -LiteralPath $baseRoot
  $baseAcl.SetAccessRule($packRead)
  Set-Acl -LiteralPath $baseRoot -AclObject $baseAcl
  Set-ItemProperty -Path $preferences -Name 'BaseDictionaryRoot' -Value $baseRoot
  # Migrate a compatible older dictionary without starting the UI or using the network.
  $resourceSettings = Join-Path $PSScriptRoot 'settings\retype.exe'
  if (Test-Path -LiteralPath $resourceSettings) {
    $resourceProcess = Start-Process -FilePath $resourceSettings -ArgumentList '--prepare-resources' -WindowStyle Hidden -Wait -PassThru
    if ($resourceProcess.ExitCode) { Write-Warning 'Base dictionary migration is incomplete; retry in Settings.' }
  }

  # The broker alone owns word-bearing learning files. AppContainers get pipe access,
  # never permission to open this directory. The native broker also enforces this ACL.
  $learning = Join-Path $env:LOCALAPPDATA 'retype\learning'
  New-Item -ItemType Directory -Path $learning -Force | Out-Null
  $learningAcl = Get-Acl -LiteralPath $learning
  $learningAcl.SetAccessRuleProtection($true, $false)
  foreach ($sid in @([Security.Principal.WindowsIdentity]::GetCurrent().User,
      [Security.Principal.SecurityIdentifier]::new('S-1-5-18'))) {
    $rule = [Security.AccessControl.FileSystemAccessRule]::new($sid,
      [Security.AccessControl.FileSystemRights]::FullControl,
      [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit',
      [Security.AccessControl.PropagationFlags]::None,
      [Security.AccessControl.AccessControlType]::Allow)
    $learningAcl.SetAccessRule($rule)
  }
  Set-Acl -LiteralPath $learning -AclObject $learningAcl
  $learningHost = Join-Path $PSScriptRoot 'retype-learning-host.exe'
  if (Test-Path -LiteralPath $learningHost) {
    # Direct native executable at user logon; no PowerShell in the typing path.
    if (-not (Test-Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run')) {
      New-Item -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' | Out-Null
    }
    Set-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'retype Learning' -Value ('"' + $learningHost + '" --serve')
    Start-Process -FilePath $learningHost -ArgumentList '--serve' -WindowStyle Hidden | Out-Null
  }
}
$flags = if ($Uninstall) { [uint32]1 } else { [uint32]0 }
foreach ($entry in $entries) {
  if ($Uninstall -and $before -notcontains $entry.Tip) { continue }
  if (-not [Retype.UserInputProfile]::InstallLayoutOrTip($entry.Tip, $flags)) {
    throw "InstallLayoutOrTip failed for $($entry.Name)."
  }
  [Retype.UserInputProfile]::SetEnabled($entry.Language, $entry.Profile, -not [bool]$Uninstall)
}
# Verify both the keyboard list and TSF enablement; never change the default.
for ($attempt = 0; $attempt -lt 10; $attempt++) {
  $after = @(Get-UserTips)
  $pending = @($entries | Where-Object { ($after -contains $_.Tip) -eq [bool]$Uninstall })
  if (-not $pending.Count) { break }
  Start-Sleep -Milliseconds 300
}
if ($pending.Count) { throw 'Windows has not reflected the requested keyboard changes in the user language list.' }
$missing = @($before | Where-Object { $tips -notcontains $_ -and $after -notcontains $_ })
if ($missing.Count) { throw "Other keyboard entries changed during registration: $($missing -join ', ')" }
Write-Host $(if ($Uninstall) { 'retype removed from the current user keyboard list.' } else { 'retype added to the current user keyboard list. Use Win+Space in a desktop app.' })
