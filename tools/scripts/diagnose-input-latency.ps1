[CmdletBinding()]
param(
    [string]$LogRoot = '',
    [ValidateRange(1, 1440)][int]$Minutes = 15,
    [switch]$AsJson
)
# Read-only numeric input timings. Never print raw log records or detail text.
$ErrorActionPreference = 'Stop'
if (-not $LogRoot) {
    try { $LogRoot = (Get-ItemProperty 'HKCU:\Software\retype').SettingsLogPath } catch {}
    if (-not $LogRoot) { $LogRoot = Join-Path $env:LOCALAPPDATA 'retype\logs' }
}
$cutoff = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() - $Minutes * 60000L
$allowedWork = @('typing', 'backspace', 'navigation', 'layout_refresh', 'choose', 'finish',
    'mode', 'boundary', 'translation', 'voice', 'symbol_backspace', 'paint')
$stages = @('total_us', 'queue_us', 'kernel_us', 'write_us', 'anchor_us', 'measure_us',
    'layout_us', 'ui_us', 'raster_us', 'notify_us', 'present_us')
$records = @(foreach ($file in @(Get-ChildItem -LiteralPath $LogRoot -Filter 'settings-*.jsonl' -File -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 12)) {
    foreach ($line in @(Get-Content -LiteralPath $file.FullName -Tail 10000 -ErrorAction SilentlyContinue)) {
        try {
            $record = $line | ConvertFrom-Json
            if ($record.role -ne 'input' -or [long]$record.time_ms -lt $cutoff -or
                $record.event -notin @('edit_timing', 'paint_timing') -or
                $record.version -notmatch '^\d+\.\d+\.\d+([+-][a-zA-Z0-9.-]+)?$') { continue }
            $detail = $record.detail | ConvertFrom-Json
            $work = if ($record.event -eq 'paint_timing') { 'paint' } else { [string]$detail.work }
            if ($work -notin $allowedWork) { continue }
            $clean = [ordered]@{ Version = [string]$record.version; Pid = [int]$record.pid
                Work = $work; Dpi = [int]$detail.dpi }
            foreach ($stage in $stages) {
                if ($null -ne $detail.$stage) {
                    $number = [double]$detail.$stage
                    if ([double]::IsNaN($number) -or [double]::IsInfinity($number) -or $number -lt 0) {
                        throw 'Invalid timing'
                    }
                    $clean[$stage] = $number
                }
            }
            [pscustomobject]$clean
        } catch { continue } # Ignore partial/old records without displaying their contents.
    }
})
$summary = @(foreach ($group in @($records | Group-Object Version, Pid, Work, Dpi |
        Sort-Object Count -Descending | Select-Object -First 8)) {
    $first = $group.Group[0]
    foreach ($stage in $stages) {
        $values = @($group.Group | ForEach-Object { if ($null -ne $_.$stage) { $_.$stage } })
        if (-not $values.Count) { continue }
        $stats = $values | Measure-Object -Average -Maximum
        [pscustomobject][ordered]@{ Version = $first.Version; Pid = $first.Pid; Work = $first.Work
            Dpi = $first.Dpi; Samples = $values.Count; Stage = $stage.Replace('_us', '')
            AverageMs = [math]::Round($stats.Average / 1000, 3)
            MaximumMs = [math]::Round($stats.Maximum / 1000, 3) }
    }
})
if ($AsJson) {
    if (-not $summary.Count) { Write-Output '[]' } else { ConvertTo-Json -InputObject $summary -Depth 3 }
    return
}
Write-Host 'Existing sampled timings only; default logs normally contain operations >= 16 ms.'
Write-Host 'These are not all-key latency percentiles, and paint timings exclude compositor display.'
if (-not $summary.Count) {
    Write-Host 'No recent input timings. For full capture, start a fresh app with RETYPE_PROFILE_INPUT=1 and reproduce.'
} else { $summary | Format-Table -AutoSize }
