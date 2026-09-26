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

## 注册（M1）

两条路，都要支持：

1. **写注册表 + regsvr32 风格**（安装器用）
   ```
   HKLM\SOFTWARE\Microsoft\CTF\TIP\{CLSID}
       \InprocServer32            (默认) = <dll 绝对路径>
                                  ThreadingModel = "Apartment"
       \LanguageProfile\0x00000804\{ProfileGUID}
                                  Enable = 1
                                  (Display Description / Description 指向资源字符串)
   ```
   64 位系统上 32 位 TIP 要走 `HKLM\SOFTWARE\WOW6432Node\...`。

2. **`ITfInputProcessorProfileMgr::RegisterProfile`**（运行时自注册，免管理员权限时用 HKCU）

**必须同时提供 x86 和 x64 两个 DLL** —— 32 位进程（很多老软件、部分游戏）只会加载 32 位 TIP，只发 x64 会导致「某些程序里完全打不出字」（test.md 第六节列的正是这类问题）。

Rust 侧需要 `i686-pc-windows-msvc` target：`rustup target add i686-pc-windows-msvc`。

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
| `ITfCandidateListUIElement` | 原生候选窗（M1 用它，M2 换自绘） | M1 |
| `ITfLangBarEventSink` | 语言栏图标点击 | M2 |

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
- 候选窗要跟随光标，位置从 `ITfContextView::GetRangeFromPoint` / `ITfRange::GetBoundingClientRect` 拿，拿不到就退回 `GetCaretPos`。
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
2. **`retype-tsf` 的 `--selftest` 模式**：一个 exe 用 `CoCreateInstance` 直接创建自己的 TIP，在自建窗口里跑 activate/key/edit 流程，不需要系统级注册。
3. 只有验证 UI 兼容性时才走「真注册 + 真应用」。

注册/注销脚本见 `platforms/windows/installer/`。
