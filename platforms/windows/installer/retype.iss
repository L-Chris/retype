; ============================================================
;  retype 输入法 —— Inno Setup 安装脚本
; ============================================================
;  由 CI 调用（也可本地调用）：
;      ISCC.exe /DMyAppVersion=0.1.0 ^
;               /DBaseDir=target\x86_64-pc-windows-msvc\release ^
;               /DRepoRoot=. /DOutDir=dist ^
;               platforms\windows\installer\retype.iss
;
;  ⚠ 本文件必须保存为 **UTF-8 with BOM**。
;    Inno Setup 6 只在有 BOM 时才按 UTF-8 解析脚本，否则中文会按 ANSI 解出乱码，
;    而且乱码不会报错 —— 它会安静地写进注册表和界面。
;
;  产物名必须是 retype-<版本>-windows-x64-setup.exe，
;  因为 core/updater 的 Platform::asset_suffix() 按这个后缀挑产物，
;  改名会让自动更新找不到安装包。
; ============================================================

; ---- 可由命令行 /D 覆盖的参数 --------------------------------
; 路径约定：**RepoRoot 必须是绝对路径**，其余三个都相对 RepoRoot。
;
; 这样约定是为了消除 Inno 的一个歧义：[Files] 的 Source 相对路径以 SourceDir
; 为基准，而 SourceDir / OutputDir / LicenseFile 各自的相对基准并不完全一致，
; 混用相对路径迟早会在某个 CI 环境里踩空。统一成「一个绝对根 + 相对子路径」就没有歧义了。
#ifndef RepoRoot
  #define RepoRoot "..\..\.."
#endif
#ifndef MyAppVersion
  ; 必须是 Inno 认可的版本号（1~4 段纯数字，每段 0~65535）。
  ; "0.0.0-ci" 这种带后缀的会让 VersionInfoVersion 直接报错。
  #define MyAppVersion "0.0.0"
#endif
#ifndef BaseDir
  #define BaseDir "target\release"
#endif
#ifndef OutDir
  #define OutDir "dist"
#endif
#ifndef X86Dir
  #define X86Dir "target\i686-pc-windows-msvc\release"
#endif
#ifndef DictFile
  #define DictFile "data\dict\retype-dict.tsv"
#endif

; ---- 固定标识 ------------------------------------------------
#define MyAppName        "retype 输入法"
#define MyAppDirName     "retype"
#define MyAppPublisher   "retype"
#define MyAppURL         "https://github.com/L-Chris/retype"

; 必须与 platforms/windows/tsf/src/ids.rs 完全一致
#define TipCLSID         "{7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52}"
#define ProfileGUID      "{A3F1C6D9-2E47-4B8A-9C51-6D0E8F2A3B74}"
#define ProfileDesc      "retype 拼音输入法（M1 本地输入预览）"

; 注册表 Subkey 里的 "{" 必须写成 "{{"，否则 Inno 运行时会把它当常量去展开并报
; "Unknown constant"。所以下面这两个 define 是**转义后**的形式，
; 不能直接拿去和 ids.rs 里的字符串比较。
#define TipRegKey        "SOFTWARE\Microsoft\CTF\TIP\{{7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52}"
#define ProfileRegKey    "SOFTWARE\Microsoft\CTF\TIP\{{7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52}\LanguageProfile\0x00000804\{{A3F1C6D9-2E47-4B8A-9C51-6D0E8F2A3B74}"

[Setup]
; AppId 是「同一个产品」的唯一标识，跨版本必须不变，否则升级会变成并存两份。
; Inno 惯例：值以 "{{" 开头，运行时渲染成单个 "{"。
AppId={{2B7F4C91-8D3E-4A67-B5F0-9C1E7D4A8B23}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}/issues
AppUpdatesURL={#MyAppURL}/releases
VersionInfoVersion={#MyAppVersion}
VersionInfoProductName={#MyAppName}
VersionInfoDescription={#ProfileDesc}
VersionInfoCompany={#MyAppPublisher}
VersionInfoOriginalFilename=retype-{#MyAppVersion}-windows-x64-setup.exe

DefaultDirName={autopf}\{#MyAppDirName}
DefaultGroupName={#MyAppDirName}
DisableProgramGroupPage=yes
DisableWelcomePage=no
AllowNoIcons=yes

OutputDir={#RepoRoot}\{#OutDir}
OutputBaseFilename=retype-{#MyAppVersion}-windows-x64-setup
; [Files] 里 Source 的相对路径统一以 RepoRoot 为基准
SourceDir={#RepoRoot}
SetupIconFile={#RepoRoot}\apps\settings\windows\runner\resources\app_icon.ico
UninstallDisplayName={#MyAppName}
UninstallDisplayIcon={code:GetPayloadDir}\retype.ico
LicenseFile={#RepoRoot}\LICENSE

Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ShowLanguageDialog=no
SetupLogging=yes

; 输入法要写 HKLM 的 CTF 键，必须管理员权限。
; 每用户安装（HKCU）是可行的，但需要另一套注册表路径，M1 再评估。
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

; 每次安装写入独立版本目录；旧应用继续持有旧 DLL，避免覆盖占用文件。
; 同版本修复也分配新目录。升级不关闭用户应用、不自动重启。
CloseApplications=no
RestartApplications=no

[Files]
Source: "platforms\windows\installer\update-*.ps1"; DestDir: "{code:GetPayloadDir}"; Flags: ignoreversion
Source: "platforms\windows\installer\refresh-hosts.ps1"; DestDir: "{code:GetPayloadDir}"; Flags: ignoreversion
Source: "apps\settings\windows\runner\resources\app_icon.ico"; DestDir: "{code:GetPayloadDir}"; DestName: "retype.ico"; Flags: ignoreversion
; Standalone Rust settings; keep the installed name compatible with older TIPs.
Source: "{#BaseDir}\retype-settings-egui.exe"; DestDir: "{code:GetPayloadDir}\settings"; DestName: "retype.exe"; Flags: ignoreversion
; 路径都相对 SourceDir（= RepoRoot）
; TSF TIP。ignoreversion 是必须的：Rust 的 cdylib 没有 VERSIONINFO 资源，
; Windows 无法按文件版本判断新旧，只能无条件覆盖。
Source: "{#BaseDir}\retype_ime.dll"; DestDir: "{code:GetPayloadDir}"; Flags: ignoreversion regserver 64bit
Source: "{#X86Dir}\retype_ime.dll"; DestDir: "{code:GetPayloadDir}\x86"; Flags: ignoreversion regserver 32bit
Source: "platforms\windows\installer\user-profile.ps1"; DestDir: "{code:GetPayloadDir}"; Flags: ignoreversion
; 更新器：自动更新的执行者（TIP DLL 自己绝不做网络 IO）
Source: "{#BaseDir}\retype-updater.exe"; DestDir: "{code:GetPayloadDir}"; Flags: ignoreversion
; Single per-user learning writer, independent of any application's TIP DLL.
Source: "{#BaseDir}\retype-learning-host.exe"; DestDir: "{code:GetPayloadDir}"; Flags: ignoreversion
; 终端调试台：M0 阶段唯一能真正体验输入链路的东西，必须有快捷方式
Source: "{#BaseDir}\retype-diag.exe"; DestDir: "{code:GetPayloadDir}"; Flags: ignoreversion
; Converts optional dictionaries downloaded later in Settings. No optional data is bundled.
Source: "{#BaseDir}\retype-dict-build.exe"; DestDir: "{code:GetPayloadDir}"; Flags: ignoreversion
; 万象 Base 已注音词库
Source: "{#DictFile}"; DestDir: "{code:GetPayloadDir}"; DestName: "retype-dict.tsv"; Flags: ignoreversion
Source: "data\dict\retype-dict.bin"; DestDir: "{code:GetPayloadDir}"; Flags: ignoreversion
; 许可与第三方数据署名（万象词库为 CC BY 4.0）
Source: "LICENSE"; DestDir: "{code:GetPayloadDir}"; Flags: ignoreversion
Source: "NOTICE.txt"; DestDir: "{code:GetPayloadDir}"; Flags: ignoreversion
Source: "data\dict\raw\wanxiang-base\LICENSE"; DestDir: "{code:GetPayloadDir}"; DestName: "LICENSE-wanxiang"; Flags: ignoreversion
Source: "data\dict\raw\LICENSE-Unicode.txt"; DestDir: "{code:GetPayloadDir}"; Flags: ignoreversion

[INI]
Filename: "{code:GetPayloadDir}\installed.ini"; Section: "Installation"; Key: "Version"; String: "{#MyAppVersion}"
Filename: "{code:GetPayloadDir}\installed.ini"; Section: "Installation"; Key: "x64"; String: "{code:GetPayloadHash64}"
Filename: "{code:GetPayloadDir}\installed.ini"; Section: "Installation"; Key: "x86"; String: "{code:GetPayloadHash32}"

[Registry]
Root: HKLM64; Subkey: "SOFTWARE\retype"; ValueType: string; ValueName: "ActiveDir"; ValueData: "{code:GetPayloadDir}"; Flags: uninsdeletevalue
Root: HKLM64; Subkey: "SOFTWARE\retype"; ValueType: string; ValueName: "Version"; ValueData: "{#MyAppVersion}"; Flags: uninsdeletevalue
; 清理旧版误写的 COM 路径；正确注册由 DLL 的 regserver 完成。
Root: HKLM; Subkey: "{#TipRegKey}\InprocServer32"; Flags: deletekey
[Icons]
Name: "{group}\retype 设置"; Filename: "{code:GetPayloadDir}\settings\retype.exe"; Comment: "调整输入方案和更新设置"
Name: "{group}\添加到当前用户的键盘列表"; Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{code:GetPayloadDir}\user-profile.ps1"""; Comment: "为当前登录用户添加 retype，不更改默认输入法"
Name: "{group}\retype 调试台"; Filename: "{code:GetPayloadDir}\retype-diag.exe"; Parameters: "--dict ""{code:GetPayloadDir}\retype-dict.tsv"""; Comment: "在终端里体验完整输入链路（不需要注销）"
Name: "{group}\检查更新"; Filename: "{code:GetPayloadDir}\settings\retype.exe"; Parameters: "--updates"; Comment: "在设置的关于页面检查更新"
Name: "{group}\许可与署名"; Filename: "{code:GetPayloadDir}\NOTICE.txt"
Name: "{group}\卸载 {#MyAppName}"; Filename: "{uninstallexe}"

[UninstallDelete]
; 安装时生成的日志
Type: filesandordirs; Name: "{app}\*.log"

[Code]
var
  DeleteUserData: Boolean;
  UserProfileAdded: Boolean;
  AutoUpdatesReady: Boolean;
  SearchRefreshReady: Boolean;
  PayloadSuffix: String;
  Previous64, Previous32: String;
  InstallCommitted, RegistrationStarted: Boolean;

function GetPayloadDir(Param: String): String;
begin
  Result := ExpandConstant('{app}\versions\{#MyAppVersion}-') + PayloadSuffix;
end;

function GetPayloadHash64(Param: String): String;
begin
  Result := GetSHA256OfFile(GetPayloadDir('') + '\retype_ime.dll');
end;

function GetPayloadHash32(Param: String): String;
begin
  Result := GetSHA256OfFile(GetPayloadDir('') + '\x86\retype_ime.dll');
end;

function InitializeSetup: Boolean;
begin
  PayloadSuffix := GetDateTimeString('yyyymmddhhnnss', '-', ':') + '-' + IntToStr(Random(1000000));
  RegQueryStringValue(HKLM64, 'Software\Classes\CLSID\{#TipCLSID}\InprocServer32', '', Previous64);
  RegQueryStringValue(HKLM32, 'Software\Classes\CLSID\{#TipCLSID}\InprocServer32', '', Previous32);
  Result := True;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Pending: String;
begin
  Result := '';
  if RegQueryMultiStringValue(HKLM64, 'SYSTEM\CurrentControlSet\Control\Session Manager',
    'PendingFileRenameOperations', Pending) then
    if Pos(Lowercase(ExpandConstant('{app}\')), Lowercase(Pending)) > 0 then
      Result := '上一次安装仍有文件等待重启替换。请先保存工作并重启，再安装此版本，避免旧注册任务覆盖新版。';
end;

procedure DeinitializeSetup;
var
  Code: Integer;
begin
  if RegistrationStarted and not InstallCommitted then begin
    if (Previous64 <> '') and FileExists(Previous64) then
      Exec(ExpandConstant('{sys}\regsvr32.exe'), '/s "' + Previous64 + '"', '', SW_HIDE, ewWaitUntilTerminated, Code);
    if (Previous32 <> '') and FileExists(Previous32) then
      Exec(ExpandConstant('{syswow64}\regsvr32.exe'), '/s "' + Previous32 + '"', '', SW_HIDE, ewWaitUntilTerminated, Code);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if CurStep = ssInstall then RegistrationStarted := True;
  if CurStep = ssPostInstall then
  begin
    // Machine COM registration and user keyboard selection are separate operations.
    // In particular, UAC may have used another administrator's account.
    UserProfileAdded := ExecAsOriginalUser(
      ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + GetPayloadDir('') + '\user-profile.ps1"',
      '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    UserProfileAdded := UserProfileAdded and (ResultCode = 0);
    Log(Format('User keyboard enrollment: %d', [ResultCode]));
    AutoUpdatesReady := ExecAsOriginalUser(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + GetPayloadDir('') + '\update-task.ps1"',
      '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    AutoUpdatesReady := AutoUpdatesReady and (ResultCode = 0);
    Log(Format('Update task enrollment: %d', [ResultCode]));
    SearchRefreshReady := ExecAsOriginalUser(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + GetPayloadDir('') + '\refresh-hosts.ps1"',
      '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    SearchRefreshReady := SearchRefreshReady and (ResultCode = 0);
    Log(Format('Search host refresh: %d', [ResultCode]));
    InstallCommitted := True;
  end;
end;

function GetCustomSetupExitCode: Integer;
begin
  Result := 0;
  if not UserProfileAdded then Result := 1;
end;

// 结束页必须说清楚两件事：
// 1) 说明选择输入法以及未自动出现时的处理方式
// 2) M1 预览的使用方法与兼容性边界
procedure CurPageChanged(CurPageID: Integer);
var
  ProfileMessage, UpdateMessage: String;
begin
  if CurPageID = wpFinished then
  begin
    if AutoUpdatesReady then
      UpdateMessage := '已启用每日更新检查，可在「设置」的「关于」页面关闭；安装新版前会征求确认。'
    else
      UpdateMessage := '自动检查任务未能创建。仍可在「设置」的「关于」页面手动检查。';
    if UserProfileAdded then
      ProfileMessage := '已加入当前用户的键盘列表。请在桌面应用中用 Win+Space 选择 retype。'
    else
      ProfileMessage := '文件已安装，但未能加入当前用户的键盘列表。请以日常使用的账户运行开始菜单中的「添加到当前用户的键盘列表」。';
    WizardForm.FinishedLabel.Caption :=
      '{#MyAppName} {#MyAppVersion} 已安装。' + #13#10 + #13#10 +
      '【选择输入法】' + #13#10 +
      ProfileMessage + #13#10 +
      UpdateMessage + #13#10 +
      'Windows 搜索等宿主已刷新；其他正在运行的应用会在下次打开时加载新版。' + #13#10 + #13#10 +
      '【当前版本的能力边界】' + #13#10 +
      'M1 预览已接入中文组字、候选窗和本地词库，包含 32 位与 64 位输入组件。' + #13#10 +
      '语言栏「中 / A」可点击切换中英，右键「设置」可切换全拼 / 小鹤双拼。' + #13#10 +
      '全拼输入 nihao，小鹤输入 nihc，空格选「你好」；1–8 或鼠标选词，- / = 翻页，Esc 取消，回车输入原拼音。' + #13#10 +
      '应用兼容性仍在验证中。';
  end;
end;

// 卸载时询问是否清掉个人词库。默认「否」：用户积累的选词习惯比一次干净卸载更值钱。
function InitializeUninstall(): Boolean;
var
  UserDataDir: String;
begin
  DeleteUserData := False;
  UserDataDir := ExpandConstant('{localappdata}\{#MyAppDirName}');
  if DirExists(UserDataDir) then
  begin
    DeleteUserData := MsgBox(
      '是否同时删除个人词库与学习记录？' + #13#10 + #13#10 +
      UserDataDir + #13#10 + #13#10 +
      '选「否」会保留，重装之后你积累的选词习惯仍然生效。',
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES;
  end;
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  ResultCode: Integer;
begin
  if CurUninstallStep = usUninstall then
  begin
    if not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + ExpandConstant('{reg:HKLM64\Software\retype,ActiveDir}\user-profile.ps1') + '" -Uninstall',
      '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
      Log('Could not start user keyboard removal');
    Log(Format('User keyboard removal: %d', [ResultCode]));
    Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      '-NoProfile -ExecutionPolicy Bypass -File "' + ExpandConstant('{reg:HKLM64\Software\retype,ActiveDir}\update-task.ps1') + '" -Uninstall',
      '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  end;
  if CurUninstallStep = usPostUninstall then
  begin
    if DeleteUserData then
      DelTree(ExpandConstant('{localappdata}\{#MyAppDirName}'), True, True, True);
    MsgBox(
      '卸载完成。' + #13#10 + #13#10 +
      '请注销并重新登录（或重启）—— ctfmon 仍缓存着已删除的输入法，' +
      '不注销的话它还会留在 Win+Space 的列表里，点了会报错。',
      mbInformation, MB_OK);
  end;
end;
