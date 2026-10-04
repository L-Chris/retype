param(
    [ValidateSet('arm64-v8a','x86_64')][string[]]$Abi = @('arm64-v8a'),
    [switch]$NativeOnly
)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
if (!$env:ANDROID_HOME) { throw 'Set ANDROID_HOME to an Android SDK with platform 35 and NDK r28+.' }
if (!$env:JAVA_HOME) { throw 'Set JAVA_HOME to JDK 17.' }
$ndk = Get-ChildItem (Join-Path $env:ANDROID_HOME 'ndk') -Directory | Sort-Object Name -Descending | Select-Object -First 1
if (!$ndk) { throw 'Install NDK r28 or newer.' }
$bin = Join-Path $ndk.FullName 'toolchains/llvm/prebuilt/windows-x86_64/bin'
if (!$env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = Join-Path $repo 'target/android' }
$env:CARGO_TARGET_DIR = [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
$savedPath = $env:PATH
$savedRustflags = $env:RUSTFLAGS
try {
    $env:PATH = "$bin;$env:JAVA_HOME\bin;$savedPath"
    Push-Location $repo
    try {
        if (!(Test-Path (Join-Path $repo 'data/dict/retype-dict.bin'))) {
            & cargo run --locked --release -p retype-dict-build -- --out data/dict/retype-dict.tsv
            if ($LASTEXITCODE -ne 0) { throw 'Dictionary build failed.' }
        }
        foreach ($architecture in $Abi) {
            $target = if ($architecture -eq 'arm64-v8a') { 'aarch64-linux-android' } else { 'x86_64-linux-android' }
            $key = $target.Replace('-', '_')
            $clangTarget = "${target}26"
            [Environment]::SetEnvironmentVariable("CC_$key", (Join-Path $bin 'clang.exe'), 'Process')
            [Environment]::SetEnvironmentVariable("AR_$key", (Join-Path $bin 'llvm-ar.exe'), 'Process')
            [Environment]::SetEnvironmentVariable("CFLAGS_$key", "--target=$clangTarget", 'Process')
            [Environment]::SetEnvironmentVariable("CARGO_TARGET_$($key.ToUpper())_LINKER", (Join-Path $bin 'clang.exe'), 'Process')
            $env:RUSTFLAGS = "-C link-arg=--target=$clangTarget -C link-arg=-Wl,-z,max-page-size=16384"
            & cargo build --locked --release -p retype-android --target $target
            if ($LASTEXITCODE -ne 0) { throw "Native build failed: $target" }
            $output = Join-Path $PSScriptRoot "build/native/$architecture"
            New-Item -ItemType Directory -Force $output | Out-Null
            Copy-Item (Join-Path $env:CARGO_TARGET_DIR "$target/release/libretype_android.so") $output
        }
    } finally { Pop-Location }
    if (!$NativeOnly) {
        Push-Location $PSScriptRoot
        try {
            & ./gradlew.bat --no-daemon :app:assembleDebug :app:testDebugUnitTest :app:lintDebug
            if ($LASTEXITCODE -ne 0) { throw 'Android build/checks failed.' }
        } finally { Pop-Location }
    }
} finally { $env:PATH = $savedPath; $env:RUSTFLAGS = $savedRustflags }
