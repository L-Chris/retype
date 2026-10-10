[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('x86_64-pc-windows-msvc', 'i686-pc-windows-msvc')][string]$Target,
    [switch]$RegisterTip
)
$ErrorActionPreference = 'Stop'
Set-Location (Resolve-Path (Join-Path $PSScriptRoot '..\..'))
$arch = if ($Target -eq 'i686-pc-windows-msvc') { 'x86' } else { 'x64' }
$targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$binaryDir = Join-Path $targetRoot "$Target\release"
$stage = ".ci\payload\$arch"
New-Item -ItemType Directory -Path $stage -Force | Out-Null

# Keep the injected DLL separate from the helpers' broker/service feature union.
$packages = @('-p', 'retype-tsf')
if ($arch -eq 'x64') { $packages += @('-p', 'retype-diag', '-p', 'retype-dict-build') }
cargo build --locked --release --target $Target @packages
if ($LASTEXITCODE) { throw 'TIP payload build failed' }
Copy-Item -LiteralPath "$binaryDir\retype_ime.dll" -Destination $stage -Force

# Compile this harness exactly once, then use it for ordinary and installed tests.
$messages = @(cargo test --locked --release --target $Target -p retype-tsf --lib --no-run --message-format=json)
if ($LASTEXITCODE) { throw 'TIP test harness build failed' }
$executables = @($messages | ForEach-Object {
    $message = $_ | ConvertFrom-Json
    if ($message.reason -eq 'compiler-artifact' -and $message.profile.test -and $message.executable) {
        $message.executable
    }
})
if ($executables.Count -ne 1) { throw 'Expected one TIP library test harness' }
$testExe = $executables[0]
& $testExe
if ($LASTEXITCODE) { throw 'TIP tests failed' }

if ($RegisterTip) {
    if ($arch -ne 'x64') { throw 'Machine registration requires the x64 runner payload' }
    $dll = Join-Path $PWD "$stage\retype_ime.dll"
    $regsvr = "$env:SystemRoot\System32\regsvr32.exe"
    $clsid = '{7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52}'
    $classKey = "Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Classes\CLSID\$clsid"
    $tipKey = "Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\CTF\TIP\$clsid"
    if ((Test-Path $classKey) -or (Test-Path $tipKey)) { throw 'Registration test needs a clean runner' }
    try {
        1..2 | ForEach-Object {
            $process = Start-Process $regsvr -ArgumentList "/s `"$dll`"" -WindowStyle Hidden -Wait -PassThru
            if ($process.ExitCode) { throw "Registration failed: $($process.ExitCode)" }
        }
        & .\platforms\windows\installer\user-profile.ps1
        & .\platforms\windows\installer\user-profile.ps1
        # The service runner has no switchable Chinese desktop input language.
        # Keep registration/enumeration/load coverage here; profile switching
        # is exercised by installed_tip_can_activate_in_test_process on a desktop.
        & $testExe installed_tip_is_discoverable_and_loadable --ignored --test-threads=1
        if ($LASTEXITCODE) { throw 'Installed TIP enumeration/loading failed' }
    } finally {
        & .\platforms\windows\installer\user-profile.ps1 -Uninstall
        $process = Start-Process $regsvr -ArgumentList "/s /u `"$dll`"" -WindowStyle Hidden -Wait -PassThru
        if ($process.ExitCode) { throw "Unregistration failed: $($process.ExitCode)" }
    }
    if ((Test-Path $classKey) -or (Test-Path $tipKey)) { throw 'Registration residue remains' }
}

if ($arch -eq 'x86') { exit 0 }
cargo build --locked --release --target $Target -p retype-updater-cli -p retype-learning -p retype-ai -p retype-sync --features retype-learning/broker,retype-ai/service
if ($LASTEXITCODE) { throw 'Helper build failed' }
foreach ($name in @('retype-diag.exe', 'retype-dict-build.exe', 'retype-updater.exe', 'retype-update-launcher.exe', 'retype-learning-host.exe', 'retype-ai-host.exe', 'retype-sync-host.exe')) {
    Copy-Item -LiteralPath "$binaryDir\$name" -Destination $stage -Force
}
# Execute the existing dictionary builder, avoiding another Cargo feature graph.
& "$stage\retype-dict-build.exe" --out data\dict\retype-dict.tsv
if ($LASTEXITCODE) { throw 'Dictionary build failed' }
Copy-Item -LiteralPath data\dict\retype-dict.tsv, data\dict\retype-dict.bin -Destination $stage -Force
foreach ($scheme in @('full', 'flypy')) {
    & "$stage\retype-diag.exe" --bench --scheme $scheme --dict "$stage\retype-dict.bin" | Tee-Object -FilePath "$stage\bench.txt" -Append
    if ($LASTEXITCODE) { throw "Benchmark failed: $scheme" }
}
if (Select-String -Path "$stage\bench.txt" -Pattern 'P99 超预算' -Quiet) { throw 'First-pass P99 exceeds 6ms' }
$repository = $env:RETYPE_GITHUB_REPO
try {
    # Do not let the runtime override mask a missing compiled-in repository.
    Remove-Item -LiteralPath Env:\RETYPE_GITHUB_REPO -ErrorAction SilentlyContinue
    & "$stage\retype-updater.exe" check --json | Tee-Object -FilePath "$stage\update-check.json"
    $code = $LASTEXITCODE
} finally {
    $env:RETYPE_GITHUB_REPO = $repository
}
if ($code -notin @(0, 2, 10)) { throw "Unexpected updater exit code: $code" }
$result = Get-Content "$stage\update-check.json" -Raw | ConvertFrom-Json
$evidence = if ($result.error) { $result.error } else { $result.release_page }
if ($evidence -and $evidence -notlike "*$repository*") { throw 'Updater points to the wrong repository' }
exit 0
