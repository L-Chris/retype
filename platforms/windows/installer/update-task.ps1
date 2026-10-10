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
$launcher = Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) 'retype-update-launcher.exe'
if (-not (Test-Path -LiteralPath $launcher)) { throw 'Native update launcher is missing.' }
$action = New-ScheduledTaskAction -Execute $launcher
$logon = New-ScheduledTaskTrigger -AtLogOn -User $identity.Name
$logon.Delay = 'PT2M'
$daily = New-ScheduledTaskTrigger -Daily -At '12:00' -RandomDelay (New-TimeSpan -Minutes 30)
$principal = New-ScheduledTaskPrincipal -UserId $identity.Name -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Hours 4)
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger @($logon,$daily) -Principal $principal -Settings $settings -Description 'Check for retype updates; installation requires user confirmation.' -Force | Out-Null
