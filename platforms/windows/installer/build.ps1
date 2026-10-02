<#
.SYNOPSIS
  一键构建 Windows 端产物：TIP DLL + 词库 + 自检。

.DESCRIPTION
  产物落在 dist\windows\：
    retype_ime.dll        TSF TIP（64 位）
    retype-dict.tsv       已注音词库
    retype-diag.exe       终端调试台

  同时产出 i686（32 位）DLL —— 32 位进程只会加载 32 位 TIP，
  只发 x64 会导致「某些程序里完全打不出字」（test.md 第六节）。

.PARAMETER SkipDict
  跳过词库构建（已有 data\dict\retype-dict.tsv 时能省 1~2 秒）

.PARAMETER NoTest
  跳过自检

.EXAMPLE
  .\build.ps1
  .\build.ps1 -SkipDict
#>
[CmdletBinding()]
param(
  [switch]$SkipDict,
  [switch]$NoTest,
  # 额外用 Inno Setup 打出安装器（需要本机装了 ISCC.exe；CI 里默认会打）
  [switch]$Installer
)

$ErrorActionPreference = 'Stop'
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..\..')
Set-Location $repoRoot

function Step($msg) { Write-Host "`n=== $msg ===" -ForegroundColor Cyan }

Step "1/5 构建 TIP DLL 与调试台（release）"
cargo build --release -p retype-tsf -p retype-diag -p retype-updater-cli -p retype-dict-build
if ($LASTEXITCODE -ne 0) { throw "cargo build 失败" }
cargo build --release -p retype-learning --features broker
if ($LASTEXITCODE -ne 0) { throw "学习后台构建失败" }
cargo build --release -p retype-ai --features service
if ($LASTEXITCODE -ne 0) { throw "AI 后台构建失败" }
cargo build --release -p retype-sync
if ($LASTEXITCODE -ne 0) { throw "云同步后台构建失败" }
rustup target add i686-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw "安装 x86 Rust target 失败" }
cargo build --release --target i686-pc-windows-msvc -p retype-tsf
if ($LASTEXITCODE -ne 0) { throw "x86 TIP 构建失败" }

$dictTsv = Join-Path $repoRoot 'data\dict\retype-dict.tsv'
if (-not $SkipDict -or -not (Test-Path $dictTsv) -or -not (Test-Path (Join-Path $repoRoot 'data\dict\retype-dict.bin'))) {
  Step "2/5 构建词库"
  $raw = Join-Path $repoRoot 'data\dict\raw\wanxiang-base\jichu.dict.yaml'
  if (-not (Test-Path $raw)) {
    throw "缺少 $raw —— 万象 Base 词库源数据未就绪"
  } else {
    cargo run --release -p retype-dict-build -- --out $dictTsv
    if ($LASTEXITCODE -ne 0) { throw "词库构建失败" }
  }
} else {
  Step "2/5 词库已存在，跳过（$([math]::Round((Get-Item $dictTsv).Length/1MB,1)) MB）"
}

if (-not $NoTest) {
  Step "3/5 自检（内核 + 真实 TSF 文本存储，不修改系统注册）"
  cargo test --workspace --release --features retype-learning/broker,retype-ai/service
  if ($LASTEXITCODE -ne 0) { throw "测试失败" }
  cargo test --release --target i686-pc-windows-msvc -p retype-tsf
  if ($LASTEXITCODE -ne 0) { throw "x86 TSF 测试失败" }
} else {
  Step "3/5 跳过自检"
}

Step "4/5 首刷延迟基准"
$diag = Join-Path $repoRoot 'target\release\retype-diag.exe'
if (Test-Path $dictTsv) {
  & $diag --bench --dict $dictTsv
} else {
  Write-Warning "没有词库，跳过基准"
}

Step "5/5 收集产物到 dist\windows"
$dist = Join-Path $repoRoot 'dist\windows'
New-Item -ItemType Directory -Force -Path $dist | Out-Null
Copy-Item (Join-Path $repoRoot 'target\release\retype_ime.dll') $dist -Force
New-Item -ItemType Directory -Force -Path (Join-Path $dist 'x86') | Out-Null
Copy-Item (Join-Path $repoRoot 'target\i686-pc-windows-msvc\release\retype_ime.dll') (Join-Path $dist 'x86') -Force
Copy-Item (Join-Path $repoRoot 'data\dict\retype-dict.bin') $dist -Force
Copy-Item $diag $dist -Force
Copy-Item (Join-Path $repoRoot 'target\release\retype-learning-host.exe') $dist -Force
Copy-Item (Join-Path $repoRoot 'target\release\retype-ai-host.exe') $dist -Force
Copy-Item (Join-Path $repoRoot 'target\release\retype-sync-host.exe') $dist -Force
Copy-Item (Join-Path $repoRoot 'target\release\retype-dict-build.exe') $dist -Force
if (Test-Path $dictTsv) { Copy-Item $dictTsv $dist -Force }
Copy-Item (Join-Path $repoRoot 'LICENSE') $dist -Force
Copy-Item (Join-Path $repoRoot 'NOTICE.txt') $dist -Force
Copy-Item (Join-Path $repoRoot 'data\dict\raw\wanxiang-base\LICENSE') (Join-Path $dist 'LICENSE-wanxiang') -Force
foreach ($f in @('retype_ime.dll', 'retype-diag.exe', 'retype-dict.tsv')) {
  $p = Join-Path $dist $f
  if (Test-Path $p) {
    Write-Host ("  {0,-22} {1,8:N1} KB" -f $f, ((Get-Item $p).Length / 1KB))
  }
}

Write-Host ""
Write-Host "下一步：" -ForegroundColor Green
Write-Host "  终端里试打字   : dist\windows\retype-diag.exe --dict dist\windows\retype-dict.tsv"
Write-Host "  打成安装器     : .\build.ps1 -Installer -SkipDict -NoTest"
Write-Host "  注册进系统(M1) : platforms\windows\installer\register.ps1（开发用；正式安装走 setup.exe）"

if ($Installer) {
  Step "6/6 构建 Inno Setup 安装器"
  cargo build --release -p retype-settings-egui
  if ($LASTEXITCODE -ne 0) { throw '设置程序构建失败' }
  $iscc = @(
    "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
    "C:\Program Files\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
  ) | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
  if (-not $iscc) {
    Write-Warning "找不到 ISCC.exe，跳过。装法: winget install JRSoftware.InnoSetup  或  choco install innosetup"
  } else {
    $ver = (Select-String -Path (Join-Path $repoRoot 'Cargo.toml') -Pattern '^version = "([^"]+)"' |
      Select-Object -First 1).Matches[0].Groups[1].Value
    Write-Host "  版本: $ver"
    # 路径约定见 retype.iss 顶部：RepoRoot 绝对，BaseDir/OutDir 相对 RepoRoot
    & $iscc "/DMyAppVersion=$ver" "/DRepoRoot=$repoRoot" `
      "/DBaseDir=target\release" "/DOutDir=dist" `
      (Join-Path $PSScriptRoot 'retype.iss')
    if ($LASTEXITCODE -ne 0) { throw "ISCC 构建失败" }
    $setup = Join-Path $repoRoot "dist\retype-$ver-windows-x64-setup.exe"
    $hash = (Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$setup.sha256", "$hash  $([IO.Path]::GetFileName($setup))`n", [Text.UTF8Encoding]::new($false))
    Get-ChildItem (Join-Path $repoRoot 'dist\*-setup.exe') | ForEach-Object {
      Write-Host ("  {0,-44} {1,8:N1} MB" -f $_.Name, ($_.Length / 1MB))
    }
  }
}
