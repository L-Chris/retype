<#
.SYNOPSIS
  Restore the pinned Wanxiang Base dictionary sources.
.DESCRIPTION
  Downloads v18.0.14, verifies the release asset hash, and extracts only the
  four dictionaries used by retype. See data/dict/raw/wanxiang-base/SOURCE.md.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$dest = Join-Path $repoRoot 'data\dict\raw\wanxiang-base'
$archive = Join-Path $env:TEMP 'retype-wanxiang-base-v18.0.14.zip'
$license = Join-Path $env:TEMP 'retype-wanxiang-base-LICENSE-v18.0.14'
$url = 'https://github.com/amzxyz/rime-wanxiang/releases/download/v18.0.14/rime-wanxiang-base.zip'
$expected = 'A41287742E2E536E96582AE36C3352E0A26D762B59D33C89D39DEB1750A6551B'
$licenseExpected = '9E5F1B3C610B9C2DA5C313BF81D577A7D1ACEC686BDB0384EDEFA6DF0F90CD94'

curl.exe -L --fail --retry 3 --silent --show-error --output $archive $url
if ($LASTEXITCODE -ne 0) { throw 'Wanxiang Base download failed' }
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash -ne $expected) {
  throw 'Wanxiang Base release asset SHA-256 mismatch'
}
curl.exe -L --fail --retry 3 --silent --show-error --output $license 'https://raw.githubusercontent.com/amzxyz/rime-wanxiang/v18.0.14/LICENSE'
if ($LASTEXITCODE -ne 0) { throw 'Wanxiang Base license download failed' }
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $license).Hash -ne $licenseExpected) {
  throw 'Wanxiang Base license SHA-256 mismatch'
}

New-Item -ItemType Directory -Force -Path $dest | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::OpenRead($archive)
try {
  foreach ($name in @('zi', 'jichu', 'lianxiang', 'duoyin')) {
    $file = "$name.dict.yaml"
    $entry = $zip.GetEntry("dicts/$file")
    if (-not $entry) { throw "Missing dictionary in release asset: $file" }
    [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $dest $file), $true)
  }
} finally {
  $zip.Dispose()
}
Copy-Item -LiteralPath $license -Destination (Join-Path $dest 'LICENSE') -Force
Write-Host "Wanxiang Base dictionary restored to $dest"
