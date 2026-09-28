[CmdletBinding()]
param([switch]$Uninstall)
$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$taskName = 'retype-update-' + $identity.User.Value
if ($Uninstall) {
  $task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
  if ($task) { $task | Unregister-ScheduledTask -Confirm:$false }
  return
}
# The task follows the active installation, rather than pinning an old version's path.
# Only code under administrator-owned Program Files is invoked.
$launcher = '$k=[Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine,[Microsoft.Win32.RegistryView]::Registry64).OpenSubKey("Software\retype"); if($k){$d=$k.GetValue("ActiveDir"); if($d){& (Join-Path $d "update-ui.ps1") -Background}}'
$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($launcher))
$action = New-ScheduledTaskAction -Execute "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -Argument "-NoProfile -STA -WindowStyle Hidden -ExecutionPolicy Bypass -EncodedCommand $encoded"
$logon = New-ScheduledTaskTrigger -AtLogOn -User $identity.Name
$logon.Delay = 'PT2M'
$daily = New-ScheduledTaskTrigger -Daily -At '12:00' -RandomDelay (New-TimeSpan -Minutes 30)
$principal = New-ScheduledTaskPrincipal -UserId $identity.Name -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Hours 4)
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger @($logon,$daily) -Principal $principal -Settings $settings -Description 'Check for retype updates; installation requires user confirmation.' -Force | Out-Null
