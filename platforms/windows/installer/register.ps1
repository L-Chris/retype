<#
.SYNOPSIS
  【开发用】注册 / 注销 retype 输入法（TSF Text Input Processor）。

.DESCRIPTION
  ⚠ 正式安装请用 Inno Setup 打出的 setup.exe（`build.ps1 -Installer`，或 CI 的 Release 产物）。
    安装器会处理 HKLM 注册、词库落位、DLL 被占用时的 Restart Manager、卸载条目等；
    本脚本只是开发期快速迭代用的轻量替代品（写 HKCU，不需要管理员权限，
    改完代码重新注册一下就能测）。

  默认按**当前用户**注册到 HKCU。
  注册后需要让 ctfmon 重新加载：注销重登，或重启 ctfmon（脚本会尝试）。

  ⚠ M0 的 DLL 是「安全骨架」：它能被系统加载、能激活、能收到按键，
    但 OnKeyDown 一律返回「不吃这个键」，因为组字串读写（ITfEditSession）是 M1 的工作。
    也就是说：现在注册它不会让你打不出字，但也不会真打出中文。
    想在真实宿主进程里观察内核行为，用影子模式：
        $env:RETYPE_TSF_SHADOW = "1"

.PARAMETER DllPath
  retype_ime.dll 的路径。默认 ..\..\..\target\release\retype_ime.dll

.PARAMETER Unregister
  注销而不是注册。

.PARAMETER Machine
  注册到 HKLM（所有用户，需要管理员权限）而不是 HKCU。

.PARAMETER DictPath
  已注音词库路径。注册时会一并复制到 %LOCALAPPDATA%\retype\，
  因为 TIP 运行在宿主进程里，工作目录不可预期。
  注意：正式安装器把词库放在 DLL 同目录，TIP 会优先从那里找（见 session.rs 的
  default_dict_path），这个复制只是给脚本安装方式留的退路。

.EXAMPLE
  .\register.ps1
  .\register.ps1 -Unregister
  .\register.ps1 -Machine -DllPath C:\retype\retype_ime.dll
#>
[CmdletBinding()]
param(
  [string]$DllPath,
  [string]$DictPath,
  [switch]$Unregister,
  [switch]$Machine
)

$ErrorActionPreference = 'Stop'

# 必须与 platforms/windows/tsf/src/ids.rs 完全一致
$CLSID   = '{7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52}'
$PROFILE = '{A3F1C6D9-2E47-4B8A-9C51-6D0E8F2A3B74}'
$LANGID  = '0x00000804'   # 简体中文
$NAME    = 'retype 输入法'
$DESC    = 'retype 拼音输入法（本地首刷 + 云端二刷）'

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..\..')

if (-not $DllPath) {
  $DllPath = Join-Path $repoRoot 'target\release\retype_ime.dll'
}
$DllPath = (Resolve-Path $DllPath -ErrorAction SilentlyContinue)
if (-not $DllPath -and -not $Unregister) {
  throw "找不到 DLL: $DllPath`n先构建: cargo build --release -p retype-tsf"
}
if ($DllPath) { $DllPath = $DllPath.Path }

$hive = if ($Machine) { 'HKEY_LOCAL_MACHINE' } else { 'HKEY_CURRENT_USER' }
# 32 位 DLL 注册到 64 位系统的 HKLM 时要走 WOW6432Node。
# 这里按 DLL 的实际位数判断，避免「装上了但 32 位程序里打不出字」。
$tipRoot = "Registry::$hive\SOFTWARE\Microsoft\CTF\TIP\$CLSID"

function Get-InstallDir {
  $base = $env:LOCALAPPDATA
  if (-not $base) { $base = $env:APPDATA }
  return (Join-Path $base 'retype')
}

function Test-Admin {
  $id = [Security.Principal.WindowsIdentity]::GetCurrent()
  (New-Object Security.Principal.WindowsPrincipal $id).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Unregister-Tip {
  if (Test-Path $tipRoot) {
    Remove-Item -LiteralPath $tipRoot -Recurse -Force
    Write-Host "已删除 $tipRoot"
  } else {
    Write-Host "注册表里没有该项，无需注销: $tipRoot"
  }
  # HKLM 下 32 位视图的残留也一并清掉
  $wow = "Registry::HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Microsoft\CTF\TIP\$CLSID"
  if (Test-Path $wow) {
    if (Test-Admin) { Remove-Item -LiteralPath $wow -Recurse -Force; Write-Host "已删除 $wow" }
    else { Write-Warning "需要管理员权限才能删除 $wow" }
  }
}

function Register-Tip {
  if ($Machine -and -not (Test-Admin)) {
    throw "-Machine 需要以管理员身份运行"
  }

  # 词库必须放到固定位置：TIP 在宿主进程里运行，工作目录是宿主的，不是我们的
  $installDir = Get-InstallDir
  New-Item -ItemType Directory -Force -Path $installDir | Out-Null
  if (-not $DictPath) {
    $DictPath = Join-Path $repoRoot 'data\dict\retype-dict.tsv'
  }
  if (Test-Path $DictPath) {
    $dest = Join-Path $installDir 'retype-dict.tsv'
    Copy-Item -LiteralPath $DictPath -Destination $dest -Force
    Write-Host "词库已复制到 $dest ($([math]::Round((Get-Item $dest).Length/1MB,1)) MB)"
  } else {
    Write-Warning "找不到词库 $DictPath —— 输入法会退化成「原样字母」模式。构建方式见 docs/dict.md"
  }

  $inproc = "$tipRoot\InprocServer32"
  New-Item -Path $inproc -Force | Out-Null
  Set-ItemProperty -LiteralPath $inproc -Name '(Default)' -Value $DllPath
  # TSF TIP 必须是 Apartment：它跑在宿主应用的 UI 线程上
  Set-ItemProperty -LiteralPath $inproc -Name 'ThreadingModel' -Value 'Apartment'

  $prof = "$tipRoot\LanguageProfile\$LANGID\$PROFILE"
  New-Item -Path $prof -Force | Out-Null
  Set-ItemProperty -LiteralPath $prof -Name 'Enable' -Value 1 -Type DWord
  Set-ItemProperty -LiteralPath $prof -Name 'Description' -Value $DESC
  Set-ItemProperty -LiteralPath $prof -Name 'Display Description' -Value $DESC

  # 显示名称（M1 应改为资源 DLL 里的字符串 ID 才能本地化）
  Set-ItemProperty -LiteralPath $tipRoot -Name 'Description' -Value $NAME -ErrorAction SilentlyContinue

  Write-Host "已注册:"
  Write-Host "  CLSID   $CLSID"
  Write-Host "  Profile $PROFILE (LCID $LANGID)"
  Write-Host "  DLL     $DllPath"
  Write-Host "  注册表  $tipRoot"
}

function Restart-Ctfmon {
  # ctfmon 会缓存 TIP 列表；不重启的话新注册的输入法要等下次登录才出现
  $p = Get-Process ctfmon -ErrorAction SilentlyContinue
  if ($p) {
    Write-Host "重启 ctfmon 以刷新输入法列表..."
    Stop-Process -Name ctfmon -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
    Start-Process "$env:SystemRoot\System32\ctfmon.exe" -ErrorAction SilentlyContinue
  }
  Write-Host ""
  Write-Host "现在用 Win+Space 或语言栏切换到「$NAME」。"
  Write-Host "如果列表里没有它，注销重登一次（TSF 的配置档缓存在会话里）。"
}

if ($Unregister) {
  Unregister-Tip
  Restart-Ctfmon
} else {
  Register-Tip
  Restart-Ctfmon
}
