[CmdletBinding()]
param([switch]$Background)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\update-common.ps1"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[Windows.Forms.Application]::EnableVisualStyles()
$mutex = New-Object Threading.Mutex($false, ('Local\retype-update-' + [Security.Principal.WindowsIdentity]::GetCurrent().User.Value))
$owned = $false
try { $owned = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $owned = $true }
if (-not $owned) { if (-not $Background) { [Windows.Forms.MessageBox]::Show('更新器已在运行，请查看已打开的更新窗口。','retype 更新') | Out-Null }; $mutex.Dispose(); return }
$timer = $null
try {
  $cache = Join-Path $env:LOCALAPPDATA 'retype\updates'
  New-Item -ItemType Directory -Path $cache -Force | Out-Null
  $statePath = Join-Path $cache 'state.json'
  $script:state = Read-UpdateState $statePath
  $script:installation = Get-RetypeInstallation
  # Reconcile an interrupted installation before deciding whether a check is due.
  if ($state.Stage -in @('installing','waiting_restart')) {
    try {
      if ($installation.Version -ne $state.TargetVersion) { throw '预期版本尚未安装。' }
      Test-RetypeInstallation $installation
      $state.Stage = 'complete'; $state.Error = ''
    } catch { $state.Stage = 'failed'; $state.Error = $_.Exception.Message }
    Save-UpdateState $state $statePath
  }
  if ($Background -and -not (Test-UpdateDue $state)) { return }
  $script:transaction = Join-Path $cache ([Guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $transaction | Out-Null
  $script:process = $null; $script:phase = ''; $script:offer = $null
  $form = New-Object Windows.Forms.Form
  $form.Text = 'retype 更新'; $form.ClientSize = New-Object Drawing.Size(560,310)
  $form.StartPosition = 'CenterScreen'; $form.FormBorderStyle = 'FixedDialog'; $form.MaximizeBox = $false
  $label = New-Object Windows.Forms.Label
  $label.SetBounds(24,20,512,64); $label.Text = "已安装版本：$($installation.Version)`r`n正在检查更新…"
  $form.Controls.Add($label)
  $details = New-Object Windows.Forms.TextBox
  $details.SetBounds(24,90,512,78); $details.Multiline = $true; $details.ReadOnly = $true; $details.ScrollBars = 'Vertical'
  $details.Text = '更新会在独立目录安装。已有应用可能继续使用旧版，重新打开应用后使用新版。'
  $form.Controls.Add($details)
  $progress = New-Object Windows.Forms.ProgressBar
  $progress.SetBounds(24,178,512,12); $progress.Style = 'Marquee'; $form.Controls.Add($progress)
  $auto = New-Object Windows.Forms.CheckBox
  $auto.SetBounds(24,207,290,24); $auto.Text = '每天自动检查并提醒（不自动安装）'; $auto.Checked = [bool]$state.AutoCheck
  $auto.Add_CheckedChanged({ $state.AutoCheck = $auto.Checked; Save-UpdateState $state $statePath }); $form.Controls.Add($auto)
  $install = New-Object Windows.Forms.Button
  $install.SetBounds(24,252,125,32); $install.Text = '下载并安装'; $install.Enabled = $false; $form.Controls.Add($install)
  $skip = New-Object Windows.Forms.Button
  $skip.SetBounds(160,252,110,32); $skip.Text = '跳过此版本'; $skip.Enabled = $false; $form.Controls.Add($skip)
  $notes = New-Object Windows.Forms.Button
  $notes.SetBounds(281,252,110,32); $notes.Text = '版本说明'; $notes.Enabled = $false; $form.Controls.Add($notes)
  $close = New-Object Windows.Forms.Button
  $close.SetBounds(402,252,134,32); $close.Text = '稍后 / 关闭'; $close.Add_Click({ $form.Close() }); $form.Controls.Add($close)
  function Start-Worker([string]$Phase, [string[]]$Arguments) {
    $script:phase = $Phase
    $script:stdout = Join-Path $transaction "$Phase.json"
    $script:stderr = Join-Path $transaction "$Phase.log"
    $script:process = Start-Process -FilePath (Join-Path $installation.Directory 'retype-updater.exe') -ArgumentList $Arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $progress.Visible = $true
  }
  function Show-Failure([string]$Message) {
    $state.Stage = 'failed'; $state.Error = $Message; Save-UpdateState $state $statePath
    $label.Text = '更新未完成。可以关闭窗口后重新检查。'; $details.Text = $Message + "`r`n日志目录：$transaction"
    $progress.Visible = $false; $close.Enabled = $true
    if (-not $form.Visible -and -not $Background) { $form.Show() }
  }
  $notes.Add_Click({ if ($offer.release_page -like 'https://github.com/L-Chris/retype/releases/*') { Start-Process $offer.release_page } })
  $skip.Add_Click({ $state.SkippedVersion = $offer.latest.version; Save-UpdateState $state $statePath; $form.Close() })
  $install.Add_Click({
    try {
      $install.Enabled = $false; $skip.Enabled = $false
      $label.Text = "正在下载并校验 $($offer.latest.version)…"
      $state.Stage = 'downloading'; $state.TargetVersion = $offer.latest.version; Save-UpdateState $state $statePath
      Start-Worker 'download' @('download','--repo','L-Chris/retype','--json','--timeout','300','--expected-version',$offer.latest.version,'--out',('"'+$transaction+'"'))
    } catch { Show-Failure $_.Exception.Message }
  })
  $timer = New-Object Windows.Forms.Timer; $timer.Interval = 300
  $timer.Add_Tick({
    if (-not $process -or -not $process.HasExited) { return }
    try {
      $code = $process.ExitCode; $process.Dispose(); $script:process = $null
      if ($phase -eq 'install') {
        if ($code -eq 3010) { $state.Stage = 'waiting_restart'; $label.Text = '安装已准备完成，需手动重启电脑后生效。' }
        elseif ($code -eq 0) {
          $new = Get-RetypeInstallation
          if ($new.Version -ne $state.TargetVersion) { throw '安装器已退出，但已安装版本不符合预期。' }
          Test-RetypeInstallation $new
          $state.Stage = 'complete'; $label.Text = "已安装 $($new.Version)，新启动的应用将使用新版。"
          $details.Text = '已有应用可能仍在使用旧版输入组件，重新打开这些应用即可。'
        } else { throw "安装器退出码：$code。详情见 $transaction\install.log" }
        Save-UpdateState $state $statePath; $progress.Visible = $false; $close.Enabled = $true
        return
      }
      $result = Get-Content -LiteralPath $stdout -Raw -Encoding UTF8 | ConvertFrom-Json
      if ($code -notin @(0,10)) { throw $(if ($result.error) { $result.error } else { "更新器退出码：$code" }) }
      if ($phase -eq 'check') {
        $script:offer = $result; $progress.Visible = $false
        if (-not $offer.update_available) {
          $state.Stage = 'up_to_date'; Save-UpdateState $state $statePath
          $label.Text = "已安装 $($installation.Version)，没有更高的稳定版本。"
          if ($Background) { $context.ExitThread() }; return
        }
        if ($Background -and $state.SkippedVersion -eq $offer.latest.version) { $context.ExitThread(); return }
        $state.Stage = 'available'; Save-UpdateState $state $statePath
        $label.Text = "已安装：$($installation.Version)`r`n发现新版本：$($offer.latest.version)"
        $install.Enabled = [bool]$offer.installable; $skip.Enabled = $true; $notes.Enabled = $true
        if (-not $offer.installable) { $details.Text = '此版本缺少安装包或校验文件，暂时无法安装。' }
        $form.Show(); $form.Activate()
      } elseif ($phase -eq 'download') {
        if (-not $result.downloaded -or $result.status.latest.version -ne $state.TargetVersion) { throw '下载结果与所选版本不一致，请重新检查。' }
        $setup = [IO.Path]::GetFullPath($result.downloaded)
        if ([IO.Path]::GetDirectoryName($setup) -ne $transaction -or [IO.Path]::GetExtension($setup) -ne '.exe') { throw '下载路径不合法。' }
        # Recheck the on-disk bytes immediately before handing off to Windows elevation.
        & (Join-Path $installation.Directory 'retype-updater.exe') verify --file $setup --json | Out-File (Join-Path $transaction 'verify.json') -Encoding UTF8
        if ($LASTEXITCODE -ne 0) { throw '安装前文件校验失败。' }
        $state.Stage = 'installing'; Save-UpdateState $state $statePath
        $label.Text = '正在安装；如弹出 Windows 管理员权限提示，请确认。'
        $script:phase = 'install'; $close.Enabled = $false
        $script:process = Start-Process -FilePath $setup -ArgumentList @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/NOCLOSEAPPLICATIONS','/NORESTARTAPPLICATIONS','/RESTARTEXITCODE=3010',('/LOG="'+$transaction+'\install.log"')) -Verb RunAs -PassThru
      }
    } catch { Show-Failure $_.Exception.Message; if ($Background -and -not $form.Visible) { $context.ExitThread() } }
  })
  $form.Add_FormClosing({ param($sender,$event)
    if ($phase -eq 'install' -and $process -and -not $process.HasExited) { $event.Cancel = $true; return }
    if ($process -and -not $process.HasExited) { $process.Kill(); $process.WaitForExit(); $process.Dispose(); $script:process = $null }
  })
  $state.LastCheck = [DateTime]::UtcNow.ToString('o'); Save-UpdateState $state $statePath
  Start-Worker 'check' @('check','--repo','L-Chris/retype','--json')
  $timer.Start()
  if (-not $Background) { $form.Show() }
  # A hidden application context permits background checks without flashing a window.
  $context = New-Object Windows.Forms.ApplicationContext
  $form.Add_FormClosed({ $context.ExitThread() })
  [Windows.Forms.Application]::Run($context)
} catch {
  if (-not $Background) { [Windows.Forms.MessageBox]::Show($_.Exception.Message,'retype 更新失败') | Out-Null }
} finally {
  if ($timer) { $timer.Stop(); $timer.Dispose() }
  if ($owned) { $mutex.ReleaseMutex() }; $mutex.Dispose()
}
