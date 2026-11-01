$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) ('retype-timing-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
try {
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $records = @('{truncated', (@{ time_ms = $now; role = 'settings'; version = '0.8.9'; detail = 'DO_NOT_EXPORT_TYPED_TEXT' } | ConvertTo-Json -Compress))
    foreach ($duration in @(20000, 40000)) {
        $records += @{ time_ms = $now; role = 'input'; event = 'edit_timing'; version = '0.8.9'; pid = 123
            detail = (@{ work = 'typing'; dpi = 144; total_us = $duration; queue_us = 1000; kernel_us = 250
                text = 'DO_NOT_EXPORT_TYPED_TEXT' } | ConvertTo-Json -Compress) } | ConvertTo-Json -Compress
    }
    $records | Set-Content -LiteralPath (Join-Path $root 'settings-input-123.jsonl') -Encoding UTF8
    $json = & (Join-Path $PSScriptRoot 'diagnose-input-latency.ps1') -LogRoot $root -AsJson
    if ($json -match 'DO_NOT_EXPORT|detail|text') { throw 'Diagnostics exported unapproved log content' }
    $rows = @($json | ConvertFrom-Json)
    $total = @($rows | Where-Object Stage -eq 'total')
    if ($total.Count -ne 1 -or $total[0].Samples -ne 2 -or $total[0].AverageMs -ne 30 -or $total[0].MaximumMs -ne 40) {
        throw 'Timing aggregation failed'
    }
    if (@($rows | Where-Object Stage -eq 'kernel')[0].AverageMs -ne 0.25) { throw 'Microsecond conversion failed' }
    Remove-Item -LiteralPath (Join-Path $root 'settings-input-123.jsonl')
    $empty = & (Join-Path $PSScriptRoot 'diagnose-input-latency.ps1') -LogRoot $root -AsJson
    if ($empty -ne '[]') { throw 'Empty timing log handling failed' }
    Write-Host 'Input timing diagnostics: aggregation, malformed records, empty logs and text exclusion passed.'
} finally { Remove-Item -LiteralPath $root -Recurse -Force }
