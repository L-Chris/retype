[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$ExpectedVersion)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\update-common.ps1"
$installation = Get-RetypeInstallation
if ($installation.Version -ne $ExpectedVersion) {
  throw "安装后版本不符合预期：$($installation.Version)；预期 $ExpectedVersion。"
}
Test-RetypeInstallation $installation
