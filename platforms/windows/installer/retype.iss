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
#define ProfileDesc      "retype 拼音输入法（本地首刷 + 云端二刷）"

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
VersionInfoProduct={#MyAppName}
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
UninstallDisplayIcon={app}\retype-diag.exe
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

; ---- DLL 正被占用时的处理 ------------------------------------
; TIP DLL 被注入到**每一个**正在运行的进程里，升级时它几乎一定是被占用的。
; Restart Manager 会列出占用进程让用户选择关闭；关不掉的由 Inno 自动登记为
; 「重启后替换」，并在结束页提示重启。这比我们自己在 M5 设计「版本化目录 +
; 注册表指向」要可靠得多 —— 那套方案已作废，见 docs/auto-update.md。
CloseApplications=yes
RestartApplications=no

[Files]
; 路径都相对 SourceDir（= RepoRoot）
; TSF TIP。ignoreversion 是必须的：Rust 的 cdylib 没有 VERSIONINFO 资源，
; Windows 无法按文件版本判断新旧，只能无条件覆盖。
Source: "{#BaseDir}\retype_ime.dll"; DestDir: "{app}"; Flags: ignoreversion
; 更新器：自动更新的执行者（TIP DLL 自己绝不做网络 IO）
Source: "{#BaseDir}\retype-updater.exe"; DestDir: "{app}"; Flags: ignoreversion
; 终端调试台：M0 阶段唯一能真正体验输入链路的东西，必须有快捷方式
Source: "{#BaseDir}\retype-diag.exe"; DestDir: "{app}"; Flags: ignoreversion
; 已注音词库（约 8.9MB，包里最大的一块）
Source: "{#DictFile}"; DestDir: "{app}"; DestName: "retype-dict.tsv"; Flags: ignoreversion
; 许可与第三方数据署名（jieba / pinyin 均为 MIT，发行时必须附带）
Source: "LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "NOTICE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "data\dict\raw\LICENSE-jieba"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; TSF 的 TIP 注册。键名里的 GUID 必须与 ids.rs 一致，否则系统找不到我们。
; ThreadingModel 必须是 Apartment —— TIP 跑在宿主应用的 UI 线程上。
Root: HKLM; Subkey: "{#TipRegKey}"; Flags: uninsdeletekeyifempty
Root: HKLM; Subkey: "{#TipRegKey}\InprocServer32"; ValueType: string; ValueName: ""; ValueData: "{app}\retype_ime.dll"; Flags: uninsdeletekey
Root: HKLM; Subkey: "{#TipRegKey}\InprocServer32"; ValueType: string; ValueName: "ThreadingModel"; ValueData: "Apartment"; Flags: uninsdeletekey
Root: HKLM; Subkey: "{#ProfileRegKey}"; ValueType: dword; ValueName: "Enable"; ValueData: "1"; Flags: uninsdeletekey
Root: HKLM; Subkey: "{#ProfileRegKey}"; ValueType: string; ValueName: "Description"; ValueData: "{#ProfileDesc}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "{#ProfileRegKey}"; ValueType: string; ValueName: "Display Description"; ValueData: "{#MyAppName}"; Flags: uninsdeletekey
; 供 M1 使用：TIP 自己也能从这里读到安装目录（虽然 dll_dir() 已经够用）
Root: HKLM; Subkey: "{#TipRegKey}"; ValueType: string; ValueName: "InstallDir"; ValueData: "{app}"; Flags: uninsdeletekey

[Icons]
Name: "{group}\retype 调试台"; Filename: "{app}\retype-diag.exe"; Parameters: "--dict ""{app}\retype-dict.tsv"""; Comment: "在终端里体验完整输入链路（不需要注销）"
Name: "{group}\检查更新"; Filename: "{app}\retype-updater.exe"; Parameters: "check"; Comment: "查询 GitHub 上的最新版本"
Name: "{group}\许可与署名"; Filename: "{app}\NOTICE.txt"
Name: "{group}\卸载 {#MyAppName}"; Filename: "{uninstallexe}"

[UninstallDelete]
; 安装时生成的日志
Type: filesandordirs; Name: "{app}\*.log"

[Code]
var
  DeleteUserData: Boolean;

// 结束页必须说清楚两件事：
// 1) TSF 的配置档被会话缓存，装完不注销是看不到输入法的（否则用户以为装失败了）
// 2) 当前里程碑的真实能力边界（M0 还不能打中文）
procedure CurPageChanged(CurPageID: Integer);
begin
  if CurPageID = wpFinished then
  begin
    WizardForm.FinishedLabel.Caption :=
      '{#MyAppName} {#MyAppVersion} 已安装。' + #13#10 + #13#10 +
      '【必须注销并重新登录】' + #13#10 +
      'TSF 的语言配置档被登录会话缓存，装完不注销是看不到输入法的。' +
      '这不是安装失败。重新登录后用 Win+Space 或语言栏切换到「{#MyAppName}」。' + #13#10 + #13#10 +
      '【当前版本的能力边界】' + #13#10 +
      '0.1.0 是地基版本：内核、352,357 条词库、拼音解码、自动更新都已可用，' +
      '但组字串读写（TSF 的 ITfEditSession）还没做，所以注册后**打不出中文**' +
      '（也不会吞掉你的按键 —— TIP 一律返回「不吃这个键」）。' + #13#10 +
      '想现在就体验完整输入链路，用开始菜单里的「retype 调试台」。';
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
begin
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
