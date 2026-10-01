param(
    [ValidateSet('glow', 'wgpu')][string]$Renderer = 'glow',
    [ValidateRange(1, 30)][int]$Rounds = 5,
    [string]$Executable,
    [string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
$taskRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Executable) { $Executable = Join-Path $taskRoot 'target\release\retype-settings-egui.exe' }
$Executable = (Resolve-Path -LiteralPath $Executable).Path
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $env:TEMP ('retype-egui-bench-' + [Guid]::NewGuid().ToString('N'))
}
[void](New-Item -ItemType Directory -Force -Path $OutputDirectory)
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path

# Include process/DLL loading before Rust main in the elapsed time, matching
# the Flutter launcher diagnostics. The DWORD clock wraps safely in the app.
if (-not ('RetypeEguiBenchmarkClock' -as [type])) {
    Add-Type -TypeDefinition @'
using System.Runtime.InteropServices;
public static class RetypeEguiBenchmarkClock {
    [DllImport("kernel32.dll")] public static extern uint GetTickCount();
}
'@
}

$samples = @()
for ($round = 1; $round -le $Rounds; $round++) {
    $report = Join-Path $OutputDirectory "$Renderer-$round.json"
    $screen = Join-Path $OutputDirectory "$Renderer.png"
    $arguments = @(
        "--renderer=$Renderer",
        "--timing-file=`"$report`"",
        '--exit-after-ms=3000',
        "--opened-at=$([RetypeEguiBenchmarkClock]::GetTickCount())"
    )
    if ($round -eq 1) { $arguments += "--screenshot=`"$screen`"" }
    $process = Start-Process -FilePath $Executable -ArgumentList $arguments -WindowStyle Hidden -PassThru
    try {
        $deadline = [DateTime]::UtcNow.AddSeconds(20)
        while (-not (Test-Path -LiteralPath $report) -and -not $process.HasExited) {
            if ([DateTime]::UtcNow -ge $deadline) { throw "Startup timed out in round $round" }
            Start-Sleep -Milliseconds 20
        }
        if (-not (Test-Path -LiteralPath $report)) {
            throw "No startup report in round $round; see $env:TEMP\retype-settings-egui-error.txt"
        }
        # Let the first screenshot and graphics allocations settle before
        # sampling process working set and private bytes, without interacting.
        Start-Sleep -Milliseconds 500
        $process.Refresh()
        $sample = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
        $sample | Add-Member -NotePropertyName working_set_mib -NotePropertyValue ([Math]::Round($process.WorkingSet64 / 1MB, 2))
        $sample | Add-Member -NotePropertyName private_mib -NotePropertyValue ([Math]::Round($process.PrivateMemorySize64 / 1MB, 2))
        $samples += $sample
        if (-not $process.WaitForExit(15000)) { throw "Settings did not exit after round $round" }
        if ($process.ExitCode -ne 0) { throw "Settings exited with code $($process.ExitCode)" }
    } finally {
        if (-not $process.HasExited) { Stop-Process -Id $process.Id }
    }
}

$summary = [ordered]@{
    renderer = $Renderer
    executable_mib = [Math]::Round((Get-Item -LiteralPath $Executable).Length / 1MB, 2)
    samples = $samples
    minimum_open_ms = ($samples.open_ms | Measure-Object -Minimum).Minimum
    maximum_open_ms = ($samples.open_ms | Measure-Object -Maximum).Maximum
    average_open_ms = [Math]::Round(($samples.open_ms | Measure-Object -Average).Average, 1)
    average_working_set_mib = [Math]::Round(($samples.working_set_mib | Measure-Object -Average).Average, 2)
    average_private_mib = [Math]::Round(($samples.private_mib | Measure-Object -Average).Average, 2)
}
$summary | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDirectory "$Renderer-summary.json") -Encoding utf8
[pscustomobject]$summary | Select-Object renderer, executable_mib, minimum_open_ms, maximum_open_ms, average_open_ms, average_working_set_mib, average_private_mib
Write-Output "Reports and screenshot: $OutputDirectory"
