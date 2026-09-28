# Windows TSF 实现笔记

本文记录**已验证**的技术事实和踩坑点，避免重复调研。

---

## 为什么不能用 Flutter / WinUI 做 Windows 输入法

Windows 输入法必须是 **TSF（Text Services Framework）Text Input Processor**：一个 COM in-proc DLL，由系统注入到**每一个**需要文本输入的进程里，实现 `ITfTextInputProcessorEx`。

- Flutter Windows 只能产出独立 `.exe`（Win32 + ANGLE/Direct3D），**无法注册为 TIP**。
- 因此 Flutter 在本项目里只承担 `apps/settings`（设置界面）。
- Android 端同理：必须是 `InputMethodService`（Kotlin），Flutter 只能做设置页。

---

## 已验证事实

验证方式：独立 probe crate（`windows` 0.62.2 + `cdylib` + `#[implement(ITfTextInputProcessorEx)]`），编译通过并产出 180KB DLL。

### 依赖与 feature

```toml
[dependencies]
windows-core = "0.62"          # 必需：#[implement] 展开引用 ::windows_core
windows-implement = "0.60"

[dependencies.windows]
version = "0.62"
features = [
  "Win32_Foundation",
  "Win32_System_Com",
  "Win32_System_Ole",
  "Win32_System_SystemServices",
  "Win32_Globalization",
  "Win32_UI_TextServices",              # TSF 全部接口
  "Win32_UI_Input_Ime",                 # 注意：不是 Win32_UI_Input_Methods（不存在）
  "Win32_UI_Input_KeyboardAndMouse",
  "Win32_UI_Shell",
  "Win32_UI_WindowsAndMessaging",
]
```

坑：
- **`windows` 0.62 没有 `implement` feature**（旧文档/旧版本才有）。`windows::core::implement` 直接可用，但必须把 `windows-core` 加为**直接依赖**，否则报 `cannot find windows_core in the crate root`。
- `BOOL` 在 0.62 是 `windows_core::BOOL`，不在 `Win32::Foundation` 下。
- feature 名写错时 cargo 会把**全部 feature 列表**打印出来（几万字符），排查时注意重定向输出。

### 关键 trait 签名（0.62.2）

```rust
pub trait ITfTextInputProcessor_Impl: windows_core::IUnknownImpl {
    fn Activate(&self, ptim: windows_core::Ref<ITfThreadMgr>, tid: u32) -> Result<()>;
    fn Deactivate(&self) -> Result<()>;
}
pub trait ITfTextInputProcessorEx_Impl: ITfTextInputProcessor_Impl {
    fn ActivateEx(&self, ptim: windows_core::Ref<ITfThreadMgr>, tid: u32, dwflags: u32) -> Result<()>;
}
pub trait IClassFactory_Impl: windows_core::IUnknownImpl {
    fn CreateInstance(&self, punkouter: Ref<IUnknown>, riid: *const GUID,
                      ppvobject: *mut *mut c_void) -> Result<()>;
    fn LockServer(&self, flock: windows_core::BOOL) -> Result<()>;
}
```

注意 `Ref<T>`（借用，可为空）而不是 `Option<&T>`；`CreateInstance` 用 `IUnknown::query(riid, ppvobject)` 填出参最省事。

### 导出函数

必须是 `extern "system"` + `#[no_mangle]`：`DllGetClassObject` / `DllRegisterServer` / `DllUnregisterServer` / `DllCanUnloadNow`。cdylib 默认会生成 `.def`，无需手写。

---

## 注册与安装排查

安装器通过 Inno Setup 的 `regserver` 调用 DLL 的 `DllRegisterServer`，卸载时调用
`DllUnregisterServer`。开发脚本 `platforms/windows/installer/register.ps1` 也走同一入口，
现在需要管理员权限（原来的 HKCU 手写注册方式不完整，已取消）。

注册包含三部分：

1. `HKLM\SOFTWARE\Classes\CLSID\{CLSID}\InprocServer32` 写入实际 DLL 路径与 `Apartment`。
2. `ITfInputProcessorProfiles::Register`、`AddLanguageProfile` 注册简体中文配置；不使用默认启用代替用户添加。
3. `ITfCategoryMgr::RegisterCategory` 注册 `GUID_TFCAT_TIP_KEYBOARD` 和 `GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT`。

不能把 COM 的 `InprocServer32` 写在 `CTF\TIP` 下。安装器会清理旧版误写的这个子键。
注册入口返回真实 HRESULT；失败时安装器会显示注册错误，开发脚本会抛出异常。

安装器在机器注册完成后，以启动安装器的原用户身份执行 `user-profile.ps1`，调用系统
`InstallLayoutOrTip` 加入当前用户键盘列表，再用 `Get-WinUserLanguageList` 验证。
不更改默认输入法，也不删除其他键盘。失败时结束页明确提示，安装器返回非零退出码。
开始菜单提供「添加到当前用户的键盘列表」，供其他账户或原用户身份不可用时补做。

修复已安装的旧版本，不需要管理员权限：

```powershell
.\platforms\windows\installer\user-profile.ps1
# 仅移除当前用户的键盘选项（不卸载 DLL）
.\platforms\windows\installer\user-profile.ps1 -Uninstall
```

卸载先移除执行卸载的账户的键盘条目，再注销机器级 COM/TSF。
其他账户的用户列表不在该卸载进程的权限上下文中；可在对应账户运行上述移除命令。
`EnableLanguageProfile` / `EnableLanguageProfileByDefault` 的 Enable 状态并不等于已加入
Windows 用户键盘列表；此前的半启用状态会导致设置页面显示异常。

为让系统搜索栏等现代及 UI-less 宿主列出 retype，注册时声明
`GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT` 与 `GUID_TFCAT_TIPCAP_UIELEMENTENABLED`；
默认安装目录中的 DLL 与只读词库位于 `Program Files` 下的同一版本目录，
供受限宿主读取。语言栏接口不可用时
继续激活按键与编辑会话；UI-less 宿主要求自己绘制候选时不创建弹窗，
即使宿主不提供光标坐标，也通过 UIElement 发送候选。
候选列表实现选择、确认和取消接口，并通过
`ITfIntegratableCandidateListUIElement` 与 `ITfFnSearchCandidateProvider`
向搜索宿主提供内联候选与只读搜索建议。安装包包含 x64 DLL 和 `x86`
子目录中的 32 位 DLL，共用根目录词库。
在干净 Windows 测试机安装新构建的安装器后，可运行只读验收：

```powershell
cargo test -p retype-tsf installed_tip_ -- --ignored --test-threads=1
```

该检查验证中文配置已启用、可枚举为键盘服务、注册了现代应用和 UI-less 类别，以及 COM 能从已注册 DLL 创建 TIP，并在测试进程内激活（不切换用户桌面的输入法）。
普通 `cargo test` 不修改系统注册；此验收默认跳过。卸载后也应检查 retype 的 COM 类、
语言配置和类别记录均已移除。

系统搜索栏还需在安装新版后的实际 Windows 会话中验收：聚焦任务栏搜索框，
用 `Win+Space` 选择 retype，输入 `nihao` 并按空格，应上屏「你好」；
关闭、重开搜索框后再次检查候选窗与切换状态。旧版 `SearchHost.exe` 若仍在运行，
需要让该宿主重新启动以加载新 DLL。

---
## 必须实现的接口清单

| 接口 | 用途 | 阶段 |
|---|---|---|
| `ITfTextInputProcessorEx` | 入口，`ActivateEx` 拿到 `ITfThreadMgr` + `tid` | M1 |
| `IClassFactory` | COM 类工厂 | M1 |
| `ITfKeyEventSink` | `OnKeyDown` / `OnKeyUp` / `OnTestKeyDown`（预判是否要吃掉这个键） | M1 |
| `ITfCompositionSink` | 组字串被宿主改动/终止的回调 | M1 |
| `ITfEditSession` | 在宿主文本存储里读写（组字、上屏） | M1 |
| `ITfDisplayAttributeProvider` | 组字串的下划线/高亮 | M1 |
| `ITfThreadFocusSink` | 焦点进出，决定候选窗显隐 | M1 |
| `ITfActiveLanguageProfileNotifySink` | 中英切换通知 | M1 |
| `ITfTextEditSink` | 宿主文本变化（用于采集上下文） | M2 |
| `ITfContextOwnerCompositionServices` | 读取光标前后文 | M2 |
| `ITfCandidateListUIElement` | 向宿主暴露候选数据；TIP 仍需绘制桌面候选窗 | M1 |
| `ITfLangBarItemButton` / `ITfLangBarItemSink` | 中/A 图标、点击切换、方案菜单及更新通知（0.1.3） | M1 |
| `ITfCompartmentEventSink` | 系统输入法开关状态同步（0.1.3） | M1 |

---

## 上屏与组字的正确姿势

```text
按键 → OnTestKeyDown 返回 fEaten=TRUE（告诉系统这个键归我）
     → OnKeyDown：
        RequestEditSession(异步 TF_ES_ASYNCDONTCARE) 里做：
          · 有组字串 → ITfRange::SetText 更新组字内容
          · 选词上屏 → 组字串 SetText(最终文字) 然后 EndComposition
          · 无组字串且是英文 → 直接放行（fEaten=FALSE）
```

坑：
- **不能在 `OnKeyDown` 里同步改文本**，必须在 edit session 里，而且优先用 `TF_ES_ASYNCDONTCARE`；同步 session（`TF_ES_SYNC`）在某些应用里会死锁。
- 候选位置在读锁内通过 `ITfContextView::GetTextExt` 获取；无布局或范围不可见时隐藏窗口。`ITfTextLayoutSink` 通知触发只读刷新。
- `Deactivate` 必须把所有 sink 反注册、把未定稿的组字串提交或取消，否则宿主应用会留下「僵尸下划线」。

---

## 稳定性红线（TIP 跑在别人进程里）

1. **所有 TSF 回调入口 `catch_unwind`**：我们 panic = Chrome 崩溃。
2. **禁止 `unwrap()` / `expect()` / 数组越界**：clippy `unwrap_used = "deny"`，`panic = "abort"` **不可用**（cdylib 要能捕获），保持默认 unwind。
3. **DLL 体积与启动开销**：会被注入到每个进程，`opt-level = "s"`、`lto = true`、`strip = true`，避免拖慢应用启动。
   实测 M0 的 release DLL 是 **1,082 KB**，比预期大得多 —— 大头是 `pinyin` crate
   内嵌的全量汉字→拼音数据（`retype-dict` 运行时要给用户自造词注音）。
   M1 的处置：构建期生成一张紧凑的「字→音节 id」表（约 30KB）随词库分发，
   用 cargo feature 把 `pinyin` crate 从 TIP 里摘掉，目标 ~300 KB。
   顺带注意：`single_char_fallback()` 会在运行时构造两万多条单字词条，
   它依赖同一份数据，摘掉 `pinyin` 之前要先把兜底词库也改成预构建产物。
4. **不要在 `DllMain` 里做任何实事**（loader lock），初始化全部放到 `ActivateEx`。
5. **线程模型是 Apartment**：不要在工作线程里直接调用主线程拿到的 COM 指针，需要跨线程用 `GIT`（Global Interface Table）或把调用 marshal 回去。**首选做法**：工作线程只算数据，所有 COM 调用回到 TSF 主线程执行。

---

## 开发回路（重要）

调 TSF 最痛的是「改一行 → 重新注册 → 注销重登 → 打开记事本试」。所以：

1. **`retype-diag`（终端调试台）**：不碰 TSF，直接在终端敲拼音验证内核与词库。90% 的迭代在这里完成。
2. **`cargo test -p retype-tsf real_tsf`**：自建内存 `ITextStoreACP`，通过真实 TSF edit session 验证文档修改，不改系统注册。`selftest.rs` 仅验证内核装配，不等同宿主上屏。
3. **`cargo run -p retype-tsf --example desktop_host`**：独立 RichEdit 测试窗口，仅在本进程激活已注册的 retype，需要先安装待测 DLL。
4. 应用兼容性仍需「真注册 + 真应用」逐项测试。

注册/注销脚本见 `platforms/windows/installer/`。
