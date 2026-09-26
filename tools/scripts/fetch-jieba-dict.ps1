<#
.SYNOPSIS
  获取 jieba 词频表（词库构建的原料）。

.DESCRIPTION
  retype 的词库原料是 jieba 的词频表（MIT 许可，见 data\dict\raw\LICENSE-jieba）。
  仓库里已经提交了一份 data\dict\raw\jieba-dict.txt，这个脚本只用于：
    · 校验现有文件的来源与完整性
    · 升级到新版 jieba-rs 后重新取一份

  做法是 `cargo fetch` 把 jieba-rs 拉到本地 registry，再从 crate 源码里取出 dict.txt。
  这样不需要联网下载任意 URL，来源可追溯到 crates.io 的校验和。

.PARAMETER Version
  jieba-rs 版本，默认 0.7

.EXAMPLE
  .\fetch-jieba-dict.ps1
  .\fetch-jieba-dict.ps1 -Version 0.7 -Force
#>
[CmdletBinding()]
param(
  [string]$Version = '0.7',
  [switch]$Force
)

$ErrorActionPreference = 'Stop'
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$dest = Join-Path $repoRoot 'data\dict\raw\jieba-dict.txt'
$licenseDest = Join-Path $repoRoot 'data\dict\raw\LICENSE-jieba'

if ((Test-Path $dest) -and -not $Force) {
  $lines = (Get-Content $dest -Encoding UTF8 | Measure-Object -Line).Lines
  Write-Host "已存在: $dest"
  Write-Host "  $lines 行, $([math]::Round((Get-Item $dest).Length/1MB,2)) MB"
  Write-Host "需要重新获取请加 -Force"
  exit 0
}

$tmp = Join-Path $env:TEMP "retype-jieba-fetch-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Force -Path (Join-Path $tmp 'src') | Out-Null
try {
  @"
[package]
name = "jieba_probe"
version = "0.0.0"
edition = "2021"

[dependencies]
jieba-rs = "$Version"
"@ | Set-Content -LiteralPath (Join-Path $tmp 'Cargo.toml') -Encoding UTF8
  '// probe' | Set-Content -LiteralPath (Join-Path $tmp 'src\lib.rs') -Encoding UTF8

  Write-Host "从 crates.io 拉取 jieba-rs $Version ..."
  Push-Location $tmp
  cargo fetch 2>&1 | Out-Null
  Pop-Location
  if ($LASTEXITCODE -ne 0) { throw "cargo fetch 失败（检查网络/代理）" }

  $reg = Join-Path $env:USERPROFILE '.cargo\registry\src'
  $crate = Get-ChildItem $reg -Directory -Recurse -Depth 1 -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -like "jieba-rs-$Version*" } |
    Sort-Object Name -Descending | Select-Object -First 1
  if (-not $crate) { throw "在 $reg 下找不到 jieba-rs-$Version*，cargo fetch 可能没成功" }

  $src = Join-Path $crate.FullName 'src\data\dict.txt'
  if (-not (Test-Path $src)) { throw "这个版本的 jieba-rs 里没有 $src（目录结构变了？）" }

  Copy-Item -LiteralPath $src -Destination $dest -Force
  $lic = Join-Path $crate.FullName 'LICENSE'
  if (Test-Path $lic) { Copy-Item -LiteralPath $lic -Destination $licenseDest -Force }

  $lines = (Get-Content $dest -Encoding UTF8 | Measure-Object -Line).Lines
  Write-Host ""
  Write-Host "已写入: $dest"
  Write-Host "  来源 : $($crate.Name) (crates.io, MIT)"
  Write-Host "  规模 : $lines 行, $([math]::Round((Get-Item $dest).Length/1MB,2)) MB"
  Write-Host "  许可 : $licenseDest"
  Write-Host ""
  Write-Host "下一步构建词库:"
  Write-Host "  cargo run -p retype-dict-build --release -- --in data/dict/raw/jieba-dict.txt --out data/dict/retype-dict.tsv"
} finally {
  Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
