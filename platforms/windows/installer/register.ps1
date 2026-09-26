<#
.SYNOPSIS
  开发用：通过 DLL 自注册入口注册/注销 retype，需要管理员权限。
.DESCRIPTION
  与正式安装器共用 COM + TSF 注册逻辑，不再手写 CTF 注册表。
  注册失败会报告 regsvr32 退出码；不会重启 ctfmon 或强制切换默认输入法。
  当前 M0 版本仍未实现中文上屏。
.EXAMPLE
  .\register.ps1 -DllPath C:\retype\retype_ime.dll
  .\register.ps1 -Unregister -DllPath C:\retype\retype_ime.dll
#>
[CmdletBinding()]
param(
  [string]$DllPath,
  [string]$DictPath,
  [switch]$Unregister,
  # 兼容旧调用；现在所有注册都需要管理员权限并写入 HKLM。
  [switch]$Machine
)
$ErrorActionPreference = 'Stop'
$id = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal $id
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw '注册和注销需要管理员权限。请在管理员 PowerShell 中运行本脚本。'
}
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
if (-not $DllPath) { $DllPath = Join-Path $repoRoot 'target\release\retype_ime.dll' }
if (-not (Test-Path -LiteralPath $DllPath -PathType Leaf)) {
  throw "找不到 DLL: $DllPath。请先运行 cargo build --release -p retype-tsf。注销也需要原 DLL。"
}
$DllPath = (Resolve-Path -LiteralPath $DllPath).Path
# 读取 PE Machine 字段，确保选用与 DLL 位数相符的 regsvr32。
$stream = [IO.File]::OpenRead($DllPath)
$reader = New-Object IO.BinaryReader $stream
try {
  if ($reader.ReadUInt16() -ne 0x5A4D) { throw '不是有效的 PE 文件' }
  $stream.Position = 0x3c
  $peOffset = $reader.ReadInt32()
  if ($peOffset -lt 0 -or $peOffset -gt ($stream.Length - 6)) { throw 'PE 头偏移无效' }
  $stream.Position = $peOffset
  if ($reader.ReadUInt32() -ne 0x4550) { throw 'PE 签名无效' }
  $arch = $reader.ReadUInt16()
} finally { $reader.Dispose() }
if ($arch -eq 0x8664) {
  if (-not [Environment]::Is64BitOperatingSystem) { throw '64 位 DLL 需要 64 位 Windows' }
  $systemDir = if ([Environment]::Is64BitProcess) { 'System32' } else { 'Sysnative' }
} elseif ($arch -eq 0x014c) {
  $systemDir = if ([Environment]::Is64BitOperatingSystem) { 'SysWOW64' } else { 'System32' }
} else { throw ('不支持的 DLL 架构: 0x{0:X4}' -f $arch) }
if (-not $Unregister) {
  if (-not $DictPath) { $DictPath = Join-Path $repoRoot 'data\dict\retype-dict.tsv' }
  if (Test-Path -LiteralPath $DictPath) {
    $installDir = Join-Path $env:LOCALAPPDATA 'retype'
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    $dest = Join-Path $installDir 'retype-dict.tsv'
    if ((Resolve-Path -LiteralPath $DictPath).Path -ne $dest) {
      Copy-Item -LiteralPath $DictPath -Destination $dest -Force
    }
  } else { Write-Warning '没有词库，内核将使用降级模式。' }
}
$regsvr = Join-Path $env:SystemRoot "$systemDir\regsvr32.exe"
if ($Unregister) { & (Join-Path $PSScriptRoot 'user-profile.ps1') -Uninstall }
$arguments = '/s ' + $(if ($Unregister) { '/u ' } else { '' }) + '"' + $DllPath + '"'
$process = Start-Process -FilePath $regsvr -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
if ($process.ExitCode -ne 0) {
  throw "DLL 注册/注销失败，regsvr32 退出码 $($process.ExitCode)。可去掉 /s 重试以查看 HRESULT。"
}
if ($Unregister) {
  Write-Host '已注销。若切换列表仍有缓存，请注销 Windows 后重新登录。'
} else {
  & (Join-Path $PSScriptRoot 'user-profile.ps1')
  Write-Host '注册成功，已加入当前执行账户的键盘列表。若使用其他管理员账户执行，请回到日常账户单独运行 user-profile.ps1。'
  Write-Host '当前 M0 版本尚不支持中文上屏；体验拼音候选请使用 retype 调试台。'
}
