[CmdletBinding()]
param([Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version)
$ErrorActionPreference = 'Stop'
Set-Location (Resolve-Path (Join-Path $PSScriptRoot '..\..'))
$candidates = @(
    'C:\Program Files (x86)\Inno Setup 6\ISCC.exe',
    'C:\Program Files\Inno Setup 6\ISCC.exe',
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
)
$iscc = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $iscc) {
    choco install innosetup --no-progress -y
    if ($LASTEXITCODE) { throw 'Inno Setup installation failed' }
    $iscc = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}
if (-not $iscc) { throw 'ISCC.exe not found' }
Copy-Item -LiteralPath .ci\payload\x64\retype-dict.tsv, .ci\payload\x64\retype-dict.bin -Destination data\dict -Force
& $iscc "/DMyAppVersion=$Version" "/DRepoRoot=$PWD" '/DBaseDir=.ci\payload\x64' '/DX86Dir=.ci\payload\x86' '/DOutDir=dist' platforms\windows\installer\retype.iss
if ($LASTEXITCODE) { throw 'Installer build failed' }
$exe = "dist\retype-$Version-windows-x64-setup.exe"
if (-not (Test-Path -LiteralPath $exe)) { throw "Missing installer: $exe" }
$suffix = (Select-String -Path core\updater\src\github.rs -Pattern 'WindowsX64 => "([^"]+)"').Matches[0].Groups[1].Value
if ($suffix -ne '-windows-x64-setup.exe' -or -not $exe.EndsWith($suffix)) { throw 'Installer/updater asset naming mismatch' }
$hash = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText((Join-Path $PWD "$exe.sha256"), "$hash  $([IO.Path]::GetFileName($exe))`n", [Text.UTF8Encoding]::new($false))
