# Shared functions; no UI, network or installation side effects when dot-sourced.
function Get-RetypeInstallation {
  $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', 'Registry64')
  try {
    $key = $base.OpenSubKey('Software\retype')
    if (-not $key) { throw 'retype installation metadata is missing.' }
    try { $directory = [string]$key.GetValue('ActiveDir'); $version = [string]$key.GetValue('Version') } finally { $key.Dispose() }
  } finally { $base.Dispose() }
  if (-not [IO.Path]::IsPathRooted($directory) -or -not (Test-Path -LiteralPath "$directory\retype-updater.exe")) { throw 'retype installation is incomplete.' }
  [pscustomobject]@{ Directory = $directory; Version = $version }
}
function Test-RetypeInstallation($Installation) {
  $manifest = Get-Content -LiteralPath (Join-Path $Installation.Directory 'installed.ini') | Where-Object { $_ -and -not $_.StartsWith('[') } | ConvertFrom-StringData
  # ConvertFrom-StringData receives one line per pipeline item; merge explicitly.
  $metadata = @{}
  foreach ($entry in $manifest) { foreach ($key in $entry.Keys) { $metadata[$key] = $entry[$key] } }
  if ($metadata.Version -ne $Installation.Version) { throw 'Installed version metadata does not match.' }
  foreach ($arch in @('x64','x86')) {
    $relative = if ($arch -eq 'x64') { 'retype_ime.dll' } else { 'x86\retype_ime.dll' }
    $dll = Join-Path $Installation.Directory $relative
    if ((Get-FileHash -LiteralPath $dll -Algorithm SHA256).Hash -ne $metadata[$arch]) { throw "Installed $arch DLL checksum mismatch." }
    $view = if ($arch -eq 'x64') { 'Registry64' } else { 'Registry32' }
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', $view)
    try {
      $key = $base.OpenSubKey('Software\Classes\CLSID\{7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52}\InprocServer32')
      if (-not $key) { throw "Missing $arch COM registration." }
      try { if ($key.GetValue('') -ne $dll) { throw "Active $arch DLL path does not match the installed version." } } finally { $key.Dispose() }
    } finally { $base.Dispose() }
  }
  $profile = Get-ItemProperty 'Registry::HKEY_LOCAL_MACHINE\Software\Microsoft\CTF\TIP\{7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52}\LanguageProfile\0x00000804\{A3F1C6D9-2E47-4B8A-9C51-6D0E8F2A3B74}'
  if ($profile.Description -ne ('retype ' + [char]0x8f93 + [char]0x5165 + [char]0x6cd5)) { throw 'Registered input method name is invalid.' }
  & (Join-Path $Installation.Directory 'user-profile.ps1') -Verify
}
function Read-UpdateState([string]$Path) {
  $state = @{ AutoCheck = $true; LastCheck = ''; SkippedVersion = ''; Stage = 'idle'; TargetVersion = ''; Error = '' }
  if (Test-Path -LiteralPath $Path) {
    try { $loaded = Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
      foreach ($key in @($state.Keys)) { if ($null -ne $loaded.$key) { $state[$key] = $loaded.$key } }
    } catch { } # A corrupt preference file must not prevent manual recovery.
  }
  return $state
}
function Save-UpdateState($State, [string]$Path) {
  $temporary = "$Path.tmp"
  [IO.File]::WriteAllText($temporary, ($State | ConvertTo-Json), (New-Object Text.UTF8Encoding($false)))
  if (Test-Path -LiteralPath $Path) { [IO.File]::Replace($temporary, $Path, [NullString]::Value) } else { [IO.File]::Move($temporary, $Path) }
}
function Test-UpdateDue($State, [DateTime]$Now = [DateTime]::UtcNow) {
  if (-not $State.AutoCheck) { return $false }
  try { return ($Now - [DateTime]::Parse($State.LastCheck).ToUniversalTime()).TotalHours -ge 24 } catch { return $true }
}
