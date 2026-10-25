# macOS 原生预览版

macOS 13 及以上，支持 Apple Silicon 和 Intel。系统接入使用 InputMethodKit，候选窗和设置使用 AppKit，共享现有 Rust 输入引擎，不需要 Flutter 或 WebView。

这是随 v0.8.0 提供的原生开发预览版，部分功能已在 Mac 真机测试，完整验收仍在进行。已接入全拼、小鹤双拼、英文补全与纠错、中文标点、个人选词学习、符号配对、在线词库、AI 翻译、语音输入、统计、WebDAV 云同步、局域网文字粘贴及更新检查／安装。

设置包含输入、词库、AI 提供商、翻译、语音输入、快捷键、统计、云同步、跨设备粘贴和关于共十个页面。延续 Windows 的浅青侧栏、白色卡片、青色开关和紧凑表单；页面标题固定，只滚动内容，始终预留细滚动条的位置。表单控件统一高度和标签对齐，异步状态刷新保留正在编辑的输入框。使用 macOS 原生窗口按钮、系统字体和 SF Symbols。

## 构建

在 Mac 安装 Xcode Command Line Tools 和当前稳定版 Rust：

```sh
xcode-select --install
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo test --locked -p retype-macos -p retype-engine
bash platforms/macos/build.sh
bash platforms/macos/test-ui.sh
```

生成 `dist/macos/retype-<version>-macos-universal.zip` 和 SHA-256 文件，包含双架构 `retype.app`、安装／卸载脚本与文档，不包含基础词库。新增的 `macOS preview` GitHub Actions 配置了桥接回归、双架构编译、plist／签名验证和独立 UI 验收程序；构建物既可作为独立预览附件下载，也随正式 Release 提供 macOS 开发预览包。

`test-ui.sh` 使用隔离的数据目录和测试设置，渲染十个页面并检查表单高度、固定标题、滚动条留白、编辑时刷新和候选尺寸；截图保存到 `target/macos-ui-validation/data/screenshots/`。它不访问个人钥匙串、网络或麦克风，也不属于正常启动流程。Windows 开发机已通过 Rust 回归测试、两种 Mac 目标的 Rust 检查和 Apple SDK 原生源码编译检查；这些检查不代表 Mac 上的运行或截图验收已完成。

本地构建默认临时签名，只适合开发测试。正式对外分发需要自己的 Developer ID 和公证：设置 `RETYPE_MACOS_SIGN_IDENTITY` 为签名身份，`RETYPE_MACOS_NOTARY_PROFILE` 为已存入钥匙串的 notarytool 配置名；构建脚本会提交公证并装订票据。代码和 CI 不保存证书或密码，默认不会发布一个已公证版本。首次启动未公证预览时可按 macOS 的「隐私与安全性」提示批准。

## 安装与使用

解压后在 Mac 双击同目录的 `install.command`，或在源码目录运行：

```sh
bash platforms/macos/install.sh /path/to/retype.app
```

安装到当前用户的 `~/Library/Input Methods/retype.app`，无需管理员权限。安装器只停止当前用户的 retype 进程，不停止其他应用；升级保留词库、个人学习和设置。随后在「系统设置 → 键盘 → 文字输入 → 编辑 → +」添加 retype；首次安装若未列出，注销后重新登录一次。不要把应用放进普通的 Applications 文件夹来替代输入法安装目录。

点击菜单栏输入法菜单的「设置…」，或双击已安装的 retype.app。首次使用在「词库」页下载基础词库，或导入发行版附件 `retype-base.retype-resources`。下载、校验、导入及数据库加载均在后台；安装前按键原样交给应用，安装完成后无需重新登录即可开始输入。

- 全拼输入 `nihao`，小鹤双拼输入 `nihc`；空格选首项，数字 1–8 选当前页，也可鼠标选词。
- `-` / `=` 翻页，上下方向键移动选择，Esc 清空；默认单独按左 Shift 切换中英文。
- 候选横排、16 点常规字重、最大宽度 480 点（Retina 自动按像素缩放），按实际字宽分页，最多 8 项；较长词组减少同页数量。
- 英文模式空格保留原始输入，Tab、数字键或鼠标接受候选并补空格。除用户配置的翻译／录音快捷键外，系统 Command / Control / Option 快捷键及 Option 组合字符交给 macOS。
- 输入方案和英文选项在下次激活输入框时生效；切换到设置前会按系统请求提交或结束组字。

## 翻译、语音和权限

在「AI 提供商」添加接口地址、API Key 和模型，可刷新模型列表或手动填写。「翻译」选择模型、目标语言、思考等级、补充要求和是否先预览；默认思考等级为不思考。默认 `Control + Option + 0` 优先翻译当前选区，没有选中文字时翻译输入框全文；自动识别原文语言并翻译成指定目标语言，提示词与 Windows、Android 共用。优先使用 InputMethodKit，在需要时通过辅助功能读取。替换前核对焦点、选区和原文，只替换捕获的范围；成功翻译但无法替换时，复制译文并显示轻量提示。

「语音输入」选择模型和识别方式。文件识别只在录音结束后发送一次识别请求，Gemini Live 支持实时预览；默认按住右 Option 录音，松开结束。设置中可独立测试录音。目前使用系统默认麦克风，设备选择请在 macOS 声音设置中完成。首次录音申请麦克风权限。提示为 12 点文字和转动图标，不增加重型窗口或关闭按钮。

「快捷键」可以重新录制切换中英文、翻译、语音输入的按键，拒绝重复绑定及系统保留组合。快捷键仅在选用 retype 输入源时处理。

在「输入」允许辅助功能访问后可启用符号配对及光标定位，也可提高部分应用的完整文本读取兼容性。拒绝权限不会阻止普通输入；安全输入框和不开放文本接口的应用仍可能限制第三方输入法访问。

## 词库、统计与同步

- 基础词库首次在线安装，支持本地包导入；扩展词库沿用 Wanxiang Base 配套分类，开启时自动下载、校验并后台编译，停用保留本地文件。不显示重复的状态标签或删除按钮。
- 统计默认开启，只保存计数和有效输入时间。中文按字、英文按词，支持最近五分钟及天／周／月／年平均速度与上期对比；语音数量单独展示。云端历史保留来源，Mac 键盘、Mac 语音、Windows 和 Android 不混算。
- WebDAV 同步沿用其他平台的协议，包含 AI 提供商、翻译／语音设置、输入选项、启用词库、学习及统计；首次合并需确认，冲突可选择本机或云端。测试连接不改变启用开关。本机云盘身份、物理快捷键和麦克风配置独立保留，Mac 快捷键使用独立同步项。
- 跨设备粘贴使用同一局域网的自动发现、六位匹配码和加密连接；任一设备可查看匹配码，另一台输入匹配码关联。关联后支持文字双向同步和重连，匹配凭证保存在钥匙串。关闭功能停止广播、发现与连接；暂不提供跨网络／Tailscale 发现。
- 关于页支持每日检查、跳过版本及用户确认后安装。更新使用 GitHub 官方附件 API，并校验文件长度、SHA-256 和应用签名；只接受含 macOS 通用安装包的稳定版本。安装器仅更新当前用户输入法目录下的 retype，不终止其他应用。从 v0.8.0 起，Release 提供 macOS 开发预览附件；当前包使用临时签名，尚未完成 Developer ID 签名和公证。

## 存储与线程

更新过程分别显示下载校验、解压检查、安装准备、保存与重启阶段。安装脚本验证就绪后才退出输入法；退出保存失败会取消安装并允许重试，替换或注册失败会尝试恢复旧应用。`logs/updates.jsonl` 记录应用更新阶段，`logs/update-install.log` 记录安装与失败原因，`updates/install-state.json` 保存最终状态；日志位于下述用户数据目录，不包含输入文本或凭证。

用户数据：`~/Library/Application Support/retype/`，其中 `dictionaries/` 为共享只读映射词库，`learning.db` 为 SQLite 个人学习，`learning.statistics/` 为数字统计日志，`sync/` 为同步状态。普通设置由当前输入法 bundle 的 UserDefaults 保存，API Key、WebDAV 密码和配对凭证使用 macOS 钥匙串。所有输入会话共享一份系统词库和学习缓存，失去焦点时释放会话及工作线程；输入进程独立运行，不向每个应用加载 DLL。

按键回调不下载文件、不打开数据库。英文纠错使用现有异步后端并检查输入代次；候选宽度由 AppKit 测量后传给内核，使分页、数字选词与显示范围一致。切换输入框结束轮询并清空旧状态，过期选词请求不生效。学习只在客户端 `insertText` 返回后入队，异常时拒绝该次学习；退出前等待已确认学习和统计写入。InputMethodKit 的插入接口没有可验证的布尔回执，因此无法确认应用内部的后续异步拒绝。系统安全输入框通常会绕过第三方输入法，仍需真机验证。

候选不抢键盘焦点；使用当前输入框的行矩形定位，限制在所属屏幕可用区域内。无位置时退回屏幕中心，防止候选落到屏幕外。没有输入内容或 API Key 的日志；桥接与客户端异常仅记录错误类别，可在 Console 中过滤进程 `retype`。

设置窗口性能日志位于 `~/Library/Application Support/retype/logs/settings-performance.jsonl`，记录页面编号、渲染／切换耗时、钥匙串读写耗时及主线程停顿；不记录表单内容、账号、API Key 或输入文本。日志达到 256 KiB 时轮换，并保留一份历史文件。密钥字段在后台读取和保存；等待授权时仍可切换页面。

## Mac 上的验收清单

当前 Windows 开发机可测试 Rust 桥接层，不能代替以下系统验收：

1. TextEdit、Safari、Chrome/Edge 和 Spotlight 中输入全拼 `nihao`、双拼 `nihc` / `wou` / `edu`，逐字更新并正常上屏。
2. 输入 `wdsssssssssss` 后连续删除到 `wd`，检查延迟、候选、焦点和重复退格。
3. 长词组、8 个短词、中英文混合候选、数字选词、`-` / `=` 翻页和鼠标选择一致。
4. 输入中切换应用、输入源、设置窗口，确认不遗留标记文本和候选；Command 快捷键及 Option 字符正常。
5. 双屏、缩放显示、Retina、全屏应用和暗色外观中检查字体清晰度及候选定位。
6. 词库未安装、断网重试、取消、导入损坏包及安装后首次输入；重复选词后退出并重开，确认学习持久化。
7. 升级、卸载、重新登录后的输入源注册，以及密码框安全输入。
8. 配置各类 AI 提供商，验证完整文本翻译、焦点变化后的复制回退；验证符号配对、越过右符号和成对退格。
9. 麦克风允许／拒绝、按住录音／松开识别、文件模式单次请求、Gemini Live 预览和设置录音测试。
10. Mac／Windows／Android 的 WebDAV 配置与学习互通、首次合并、冲突处理、统计来源隔离；局域网双向配对、复制、重连及停用。
11. 正式签名包发布后验证更新检查、校验失败重试、安装、回滚与升级后输入源可用。

卸载：先选其他输入源并从系统输入源列表移除 retype，然后运行 `bash platforms/macos/uninstall.sh`。脚本会确认后只删除应用，保留个人数据。

接入依据：[Apple InputMethodKit](https://developer.apple.com/documentation/inputmethodkit)、[IMKServer](https://developer.apple.com/documentation/inputmethodkit/imkserver)、[IMKInputController](https://developer.apple.com/documentation/inputmethodkit/imkinputcontroller)；输入法 bundle 配置参考 [Mozc 的原生 macOS 宿主](https://github.com/google/mozc/blob/master/src/mac/Info.plist)。
