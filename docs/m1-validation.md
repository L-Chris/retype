# M1 桌面输入预览验证

以下记录对应 0.1.3 桌面预览版。本轮本地验证使用隔离 DLL，没有覆盖当前系统安装的 DLL。

## 使用

运行 `dist/retype-0.1.3-windows-x64-setup.exe` 覆盖安装，无需先卸载。保存工作后重新打开使用输入法的应用；若安装器要求重启，按提示操作。安装包包含 x64 和 x86 TIP，以及共用的二进制词库。

在桌面应用中用 Win+Space 选择 retype：

- 点击语言栏「中 / A」切换中英，右键菜单选择全拼或小鹤双拼。方案保存在 `HKCU\Software\retype\PinyinScheme`（0 全拼，1 小鹤），其他窗口重新获得焦点或激活时同步。
- 全拼输入 `nihao`，小鹤输入 `nihc`，空格选择「你好」；1–5 或鼠标选择当前页候选。
- 左右键移动候选选择，PgUp/PgDn 翻页，Backspace 删除拼音。
- Esc 取消未定稿输入；Enter 提交原始拼音。
- Ctrl+Space 已注册为 TSF 保留快捷键，但本机自动化未验证触发成功；请使用图标点击切换。刚加载词库时可能短暂显示原始字母候选。

## 已验证

- 独立 RichEdit 窗口，加载待测 DLL，真实按键逐字输入 `nihao`：组字串随输入变化，候选包含「你好」，空格上屏为「你好」且候选窗关闭。随后输入 `n`、Esc，文档保持「你好」。
- 0.1.3：真实按键输入 `nihc`，紧凑横排窗出现「你好」；鼠标点击后上屏，输入框保留焦点。按键和鼠标选词通过同一 TSF edit session，过期候选点击被丢弃。
- 测试宿主从实际 `ITfLangBarItemMgr` 取回模式项，在标题栏呈现该项返回的图标；点击模式菜单后图标从「中」变成「A」，英文 `a` 直通、不显示候选。全拼/小鹤菜单改变实际内核方案，重启宿主后保留小鹤。这个接口测试不等同所有 Windows 任务栏样式已验收。
- 系统 `GUID_COMPARTMENT_KEYBOARD_OPENCLOSE` 变化同步到内核的 COM 测试通过。
- 测试期间使用临时当前用户 COM 路径覆盖加载隔离目录 DLL；覆盖已清理，测试窗口已关闭，机器级安装路径未变。
- `cargo test --workspace --release`：216 项通过；`cargo test --release --target i686-pc-windows-msvc -p retype-tsf`：28 项通过，各有 2 项安装注册验收默认跳过。构建日志在 `target/m1-013-build.log`。
- `cargo clippy --workspace --all-targets -- -D warnings` 和 `cargo fmt --all -- --check`。
- 真实 `ITextStoreACP`/TSF 测试覆盖组字、空格提交、Esc 取消、标点提交、0 的原文处理、拒绝写锁、异步授锁，以及焦点变化后丢弃过期键。内核在写锁授予前不消费按键。
- 352,357 条二进制词库回读；截断、无效格式等错误输入拒绝加载。

## 尚未完成

- 记事本、Office、Chrome、VS Code、Terminal、微信/QQ 的逐项兼容性矩阵；独立 RichEdit 测试不等同这些应用验收。
- 现代应用/AppContainer 支持，当前保留「仅桌面」限制。
- 不同 Windows 任务栏配置、高 DPI/多显示器外观、暗色/高对比度主题、Ctrl+Space 在真实宿主中的兼容性。
- DLL 尚未达到路线图的约 300 KB 目标。`pinyin` 已移到构建依赖，运行时采用紧凑注音表。

## 候选 UI 方案

本版采用原生 Win32/GDI：轻量、不抢焦点，五项横排、圆角和蓝色高亮，长候选限制宽度并省略显示。后续可迁移到 Direct2D/DirectWrite 改善缩放与文字绘制；Flutter/WebView 更适合设置页，候选窗无需引入这些运行时。TSF UIElement 继续用于宿主显隐协商及候选枚举。

候选接口与键盘翻译依据：[Microsoft UI-less mode](https://learn.microsoft.com/en-us/windows/win32/tsf/uiless-mode-overview)、[ToUnicodeEx](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-tounicodeex)。`ITfCandidateListUIElement` 提供数据和显隐协商，不自动绘制候选窗口；键盘预判使用不改变死键状态的转换选项。
