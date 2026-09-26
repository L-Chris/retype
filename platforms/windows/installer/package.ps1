<#
.SYNOPSIS
  打包 Windows 发行产物：zip + sha256。

.DESCRIPTION
  CI 与本地共用同一个脚本，避免「CI 打的包和本地打的不一样」。
  产物命名必须与 core/updater 的 Platform::asset_suffix() 保持一致，
  否则更新器挑不到产物：
      retype-<版本>-windows-<arch>.zip
      retype-<版本>-windows-<arch>.zip.sha256

  构建（cargo build）由调用方负责，本脚本只做收集 + 压缩 + 校验和。

.PARAMETER Version
  版本号（不带 v 前缀）。CI 从 git tag 推导。

.PARAMETER Arch
  x64（默认）或 x86。

.PARAMETER TargetDir
  cargo 的 target 目录，默认 target。带 --target 时是 target\<triple>。

.EXAMPLE
  .\package.ps1 -Version 0.1.0
  .\package.ps1 -Version 0.1.0 -Arch x86 -TargetDir target\i686-pc-windows-msvc
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory)][string]$Version,
  [ValidateSet('x64', 'x86')][string]$Arch = 'x64',
  [string]$TargetDir = 'target',
  [string]$OutDir = 'dist'
)

$ErrorActionPreference = 'Stop'
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..\..')
Set-Location $repoRoot

$Version = $Version.TrimStart('v', 'V')
$name = "retype-$Version-windows-$Arch"
$rel = Join-Path $TargetDir 'release'
$stage = Join-Path $OutDir "stage\$name"

function Need($path, $what) {
  if (-not (Test-Path $path)) {
    throw "缺少 $what`: $path`n先执行: cargo build --release -p retype-tsf -p retype-diag -p retype-updater-cli"
  }
}

Write-Host "=== 打包 $name ==="

# 1. 收集二进制
Need (Join-Path $rel 'retype_ime.dll') 'TIP DLL'
Need (Join-Path $rel 'retype-diag.exe') '调试台'
Need (Join-Path $rel 'retype-updater.exe') '更新器'
Need 'data\dict\retype-dict.tsv' '词库（cargo run -p retype-dict-build 生成）'

if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

$bins = @(
  @{ src = Join-Path $rel 'retype_ime.dll';    what = 'TSF TIP DLL' },
  @{ src = Join-Path $rel 'retype-diag.exe';   what = '终端调试台' },
  @{ src = Join-Path $rel 'retype-updater.exe'; what = '更新器' }
)
foreach ($b in $bins) {
  Copy-Item $b.src $stage -Force
  Write-Host ("  {0,-22} {1,9:N0} KB  {2}" -f (Split-Path $b.src -Leaf), ((Get-Item $b.src).Length / 1KB), $b.what)
}

# 2. 词库（8~9MB，是包里最大的一块）
Copy-Item 'data\dict\retype-dict.tsv' $stage -Force
Write-Host ("  {0,-22} {1,9:N0} KB  已注音词库" -f 'retype-dict.tsv', ((Get-Item 'data\dict\retype-dict.tsv').Length / 1KB))

# 3. 安装脚本：register.ps1 原样复制，install/uninstall 是薄包装
Copy-Item 'platforms\windows\installer\register.ps1' $stage -Force

@'
# 以当前用户身份安装 retype 输入法（不需要管理员权限）
$ErrorActionPreference = 'Stop'
& (Join-Path $PSScriptRoot 'register.ps1') `
    -DllPath  (Join-Path $PSScriptRoot 'retype_ime.dll') `
    -DictPath (Join-Path $PSScriptRoot 'retype-dict.tsv')
'@ | Set-Content -LiteralPath (Join-Path $stage 'install.ps1') -Encoding UTF8

@'
# 卸载 retype 输入法
$ErrorActionPreference = 'Stop'
& (Join-Path $PSScriptRoot 'register.ps1') -Unregister
'@ | Set-Content -LiteralPath (Join-Path $stage 'uninstall.ps1') -Encoding UTF8

# 4. 许可与说明
if (Test-Path 'LICENSE') { Copy-Item 'LICENSE' $stage -Force }
Copy-Item 'NOTICE.txt' $stage -Force
Copy-Item 'data\dict\raw\LICENSE-jieba' $stage -Force

@"
retype $Version (windows-$Arch)
================================

【这是什么】
一个拼音输入法的 M0 骨架。内核、词库、拼音解码、更新器都已可用，
但 **还不能在系统里打中文** —— 组字串读写（TSF 的 ITfEditSession）
是下一个里程碑（M1）的工作。现在注册它不会吞掉你的按键
（TIP 一律返回「不吃这个键」），但也打不出中文。

【现在能做什么】
1) 在终端里试完整的输入链路（推荐，不需要安装）：
     retype-diag.exe --dict retype-dict.tsv
   输入拼音回车即可看候选；:help 看全部命令；
   :voice <文字> 可以模拟一次完整的语音三段式流程。

2) 看首刷延迟基准：
     retype-diag.exe --dict retype-dict.tsv --bench

3) 排查候选排序：
     retype-diag.exe --dict retype-dict.tsv --explain nihaomashijie

4) 检查更新：
     retype-updater.exe check --repo <owner/name>

【安装到系统（M1 起才有实际意义）】
     powershell -ExecutionPolicy Bypass -File install.ps1
   卸载：
     powershell -ExecutionPolicy Bypass -File uninstall.ps1

【校验】
本包附带 .sha256。校验命令：
     certutil -hashfile retype-$Version-windows-$Arch.zip SHA256
   与 retype-$Version-windows-$Arch.zip.sha256 里的值比对。

更多信息见仓库的 README.md 与 ARCHITECTURE.md。
"@ | Set-Content -LiteralPath (Join-Path $stage 'README.txt') -Encoding UTF8

# 5. 压缩
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$zip = Join-Path $OutDir "$name.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Write-Host "  压缩中..."
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal

# 6. sha256 旁文件。格式必须是 coreutils 的 `<hex>  <文件名>`，
#    core/updater 的 parse_sha256sum 按这个格式解析。
#    只写基名不写路径：下载方拿到的是基名，带路径会匹配不上。
$hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLower()
$sumPath = Join-Path (Resolve-Path $OutDir).Path "$name.zip.sha256"
[System.IO.File]::WriteAllText(
  $sumPath, "$hash  $name.zip`n", (New-Object System.Text.UTF8Encoding($false)))

Write-Host ""
Write-Host ("  {0,-40} {1,9:N1} MB" -f "$name.zip", ((Get-Item $zip).Length / 1MB))
Write-Host ("  {0,-40} {1}" -f "$name.zip.sha256", $hash)
Write-Host ""
Write-Host "打包完成: $((Resolve-Path $OutDir).Path)"

# 清理暂存目录，避免污染工作区
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
