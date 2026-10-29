param(
    [string]$Pinyin = 'laqu',
    [switch]$Once,
    [string]$PipeName = ''
)
# Read-only: no learning events, process restarts, database edits, or history export.
$ErrorActionPreference = 'Stop'
if ($Pinyin -notmatch "^[a-z']{1,64}$") { throw 'Use lowercase pinyin letters/apostrophes.' }
$normalized = $Pinyin.Replace("'", '')
$isWindowsHost = $env:OS -eq 'Windows_NT'
if (-not $PipeName) {
    if (-not $isWindowsHost) { throw 'Run this script on the Windows test computer.' }
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $PipeName = "retype-learning-$sid-v1"
}
function Read-Exact($Stream, [int]$Length) {
    $buffer = New-Object byte[] $Length
    $offset = 0
    while ($offset -lt $Length) {
        $task = $Stream.ReadAsync($buffer, $offset, $Length - $offset)
        if (-not $task.Wait(5000)) { $Stream.Dispose(); throw 'Learning pipe read timed out.' }
        $count = $task.Result
        if ($count -le 0) { throw 'Learning pipe closed before the response completed.' }
        $offset += $count
    }
    return ,$buffer
}
function Get-LearningSample {
    $pipe = [IO.Pipes.NamedPipeClientStream]::new('.', $PipeName,
        [IO.Pipes.PipeDirection]::InOut, [IO.Pipes.PipeOptions]::Asynchronous,
        [Security.Principal.TokenImpersonationLevel]::Identification)
    try {
        $pipe.Connect(2000)
        $request = [ordered]@{
            version = 1; client = "diagnose-$PID-$([Guid]::NewGuid().ToString('N'))"
            known_revision = $null; events = @(); stop = $false
        } | ConvertTo-Json -Compress
        $bytes = [Text.Encoding]::UTF8.GetBytes($request)
        foreach ($part in @([BitConverter]::GetBytes([uint32]$bytes.Length), $bytes)) {
            $task = $pipe.WriteAsync($part, 0, $part.Length)
            if (-not $task.Wait(5000)) { $pipe.Dispose(); throw 'Learning pipe write timed out.' }
        }
        $header = Read-Exact $pipe 4
        $length = [BitConverter]::ToUInt32($header, 0)
        if ($length -gt 32 * 1024 * 1024) { throw 'Learning response exceeds the protocol limit.' }
        $data = Read-Exact $pipe $length
        $response = [Text.Encoding]::UTF8.GetString($data) | ConvertFrom-Json
        if ($response.version -ne 1 -or $null -eq $response.snapshot) { throw 'No supported learning snapshot returned.' }
        $snapshot = $response.snapshot
        $words = @($snapshot.ranking.records | Where-Object {
            ($_.pinyin -replace "[' ]", '') -eq $normalized
        } | Select-Object text, count, recent, last_tick, prior_logp)
        $legacy = @($snapshot.entries | Where-Object {
            ($_.pinyin -replace "[' ]", '') -eq $normalized
        } | Select-Object text, logp)
        $english = @($snapshot.english | Where-Object { $_[0] -ieq $normalized } | ForEach-Object {
            [pscustomobject]@{ text = $_[0]; count = $_[1] }
        })
        [ordered]@{ revision = $response.revision; tick = $snapshot.ranking.tick
            words = $words; entries = $legacy; english = $english }
    } finally { $pipe.Dispose() }
}
$install = [ordered]@{}
if ($isWindowsHost) {
    try { $install.active_dir = (Get-ItemProperty 'HKLM:\SOFTWARE\retype').ActiveDir } catch {}
    try { $install.dictionary_root = (Get-ItemProperty 'HKCU:\SOFTWARE\retype').BaseDictionaryRoot } catch {}
    try {
        $install.learning_hosts = @(Get-CimInstance Win32_Process -Filter "Name='retype-learning-host.exe'" |
            Select-Object ProcessId, ExecutablePath)
    } catch { $install.host_query_error = $_.Exception.Message }
    if (-not $install.dictionary_root -and $env:LOCALAPPDATA) {
        $install.dictionary_root = Join-Path $env:LOCALAPPDATA 'retype\dictionaries'
    }
    if ($install.dictionary_root) {
        $install.dictionary_files = @(Get-ChildItem -LiteralPath $install.dictionary_root -Filter '*.bin' -File -ErrorAction SilentlyContinue |
            Select-Object -First 8 Name, Length, LastWriteTimeUtc)
    }
    $install.loaded_imes = @(foreach ($process in Get-Process -Name notepad, chrome, msedge, Code, explorer, SearchHost -ErrorAction SilentlyContinue) {
        try { foreach ($module in $process.Modules) {
            if ($module.ModuleName -eq 'retype_ime.dll') {
                [pscustomobject]@{ process = $process.ProcessName; pid = $process.Id; dll = $module.FileName }
            }
        } } catch {} # Some protected/elevated processes cannot be inspected.
    })
    $dataRoot = Join-Path $env:LOCALAPPDATA 'retype\learning'
    $install.database_files = @(Get-ChildItem -LiteralPath $dataRoot -Filter 'user.db*' -ErrorAction SilentlyContinue |
        Select-Object Name, Length, LastWriteTimeUtc)
}
function Get-SelectionMetadata {
    if (-not $isWindowsHost) { return @() }
    $logRoot = Join-Path $env:LOCALAPPDATA 'retype\logs'
    # The shared logger can use the first caller's role in its filename, so
    # learning records may live in settings-launcher-*.jsonl as well.
    @(Get-ChildItem -LiteralPath $logRoot -Filter 'settings-*.jsonl' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 6 | ForEach-Object {
            Get-Content -LiteralPath $_.FullName -Tail 20 -ErrorAction SilentlyContinue | ForEach-Object {
                try {
                    $entry = $_ | ConvertFrom-Json
                    if ($entry.event -eq 'selection') {
                        $detail = $entry.detail | ConvertFrom-Json
                        [pscustomobject]@{ time_ms = $entry.time_ms; pid = $entry.pid
                            version = $entry.version; events = $detail.events
                            allowed = $detail.allowed; commits = $detail.commits; write_result = $detail.write_result }
                    }
                } catch {}
            }
        })
}
$report = [ordered]@{ pinyin = $Pinyin; install = $install }
try { $report.before = Get-LearningSample } catch { $report.before_error = $_.Exception.Message }
if (-not $Once) {
    Write-Host 'In another normal text field, type the pinyin and explicitly select the WHOLE intended word 5 times.'
    [void](Read-Host 'Return here and press Enter after those selections')
    Start-Sleep -Seconds 2 # Allow the normal asynchronous broker flush.
    try { $report.after = Get-LearningSample } catch { $report.after_error = $_.Exception.Message }
}
$report.selection_metadata = @(Get-SelectionMetadata)
$report | ConvertTo-Json -Depth 10
