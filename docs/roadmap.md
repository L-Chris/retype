# 路线图

每个里程碑都有**可验证的验收标准**（不接受「应该能用」）。

---

## M0 · 地基（已完成）

目标：把架构落成能编译、能测试的代码骨架，验证最高风险的技术选型。

- [x] 纯 Rust 实现 TSF COM 骨架可编译并产出 DLL（见 [windows-tsf.md](./windows-tsf.md#已验证事实)）
- [x] monorepo 布局 + Cargo workspace
- [x] 本地拼音引擎：音节切分 / 词格 / Viterbi k-best，单元测试覆盖切分歧义（`xian` / `shanghai` / `jian`）
- [x] 云端抽象 + Mock 实现（`CloudPinyin` / `LlmReranker`）
- [x] 统一内核：首刷 → 二刷 → 合并 → `gen` 过期丢弃
- [x] `retype-diag` 终端调试台：敲拼音看候选
- [x] 真实词库跑通：349,045 词 → 352,357 条已注音词条
- [x] 首刷延迟基准达标（P50 445µs / P99 3.35ms，预算 5ms）
- [x] 版本号统一为 0.1.0（Cargo workspace 为唯一真源）
- [x] CI：`ci.yml`（PR/main）+ `release.yml`（tag `v*` → 构建 + 打包 + 发布 Release）
- [x] 自动更新：`core/updater` + `retype-updater.exe`（check / download + sha256 校验）

**验收**：`cargo test --workspace` 全绿（204 项）；`retype-diag` 里输入 `nihaomashijie`
能得到「你好吗世界」；`cargo clippy --workspace --all-targets -- -D warnings` 零警告；
打 tag 后 CI 产出 `retype-<版本>-windows-x64-setup.exe`（Inno Setup 安装包）+ `.sha256` 并发布 Release。

**已端到端验证**（仓库 [L-Chris/retype](https://github.com/L-Chris/retype)，v0.1.0）：

| 环节 | 结果 |
|---|---|
| `release.yml` 由 tag 触发 | ✅ v0.1.0 3m15s、v0.1.1 4m46s，均发布 Release |
| 产物命名与 `Platform::asset_suffix()` 对齐 | ✅ v0.1.1 起为 `retype-<版本>-windows-x64-setup.exe` + 同名 `.sha256`（Inno Setup 安装包） |
| `ci.yml` 四个 job | ✅ 全绿（ubuntu 内核 / windows 全量 / Flutter / 版本一致性） |
| 内核在 **ubuntu** 上编译通过 | ✅ 证明 `core/` 确实不含任何平台 API |
| `retype-updater check` 对真实 release | ✅ `--current 0.0.9` → 退出码 10 且「可安装 = true」；`--current 0.1.0` → 退出码 0，不提示降级 |
| `retype-updater download` + sha256 校验 | ✅ 从真实 Release 下载 5,783,576 字节，校验通过后落盘 |
| **真实升级路径** | ✅ 0.1.0 → 0.1.1：check 判定有更新且可安装，download 拉取并校验通过 |
| 篡改检测 | ✅ 改动 1 字节后 `verify` 退出码 4 并拒绝安装 |
| Inno Setup 安装器 | ✅ 本地 6.7.3 与 CI 均编译通过（5.5MB），7 个文件全部打包 |

踩到的坑（都已修）：
- `cargo clippy -- -D warnings -p xxx` 里的 `-p` 在 `--` 之后会被当成 rustc 参数。
- Inno 的 `VersionInfoVersion` 不接受 `0.0.0-ci` 这种带后缀的版本号（必须 1~4 段纯数字）。
- `VersionInfoProduct` 不是 Inno 指令，正确名是 `VersionInfoProductName`。
- `[Files]` 的相对路径基准有歧义，统一成「RepoRoot 绝对 + 其余相对 RepoRoot」并显式 `SourceDir`。
- 验证安装器时**不要跑 `setup.exe /?`**：Inno 对 `/?` 的响应是弹模态帮助框，会挡住自动化流程。

---

## M1 · 能在 Windows 里打中文（桌面预览，验收中）

目标：真正装进系统，在主流应用里可用。这是**产品能不能活下来**的一关（test.md 第六节）。

- [x] TSF 基础管线：`ITfKeyEventSink`（含无副作用预判）、`ITfEditSession`、`ITfCompositionSink`、`ITfTextEditSink`、`ITfThreadFocusSink`；延迟写锁、拒绝写锁和过期请求已测试
- [x] 注册与激活：TSF 中文配置、`ITfLangBarItemButton` 中/A 状态图标、模式通知、方案菜单与保留快捷键
- [x] 组字串显示：`ITfDisplayAttributeProvider` 提供下划线属性，宿主负责绘制
- [x] 候选窗：`ITfCandidateListUIElement` 暴露候选数据，桌面无激活窗口负责显示；响应宿主 Show/Hide 与布局变化。此接口不会自动替输入法绘制窗口
- [x] 系统词库：`tools/dict-build` 产出 `retype-dict.bin`，随 DLL 分发，异步加载；保留 TSV 调试用途
- [x] x86 + x64 双架构产出并装入同一个 x64 Windows 安装包（32 位应用加载 x86 TIP）
- [ ] **把 TIP DLL 从 1,082 KB 压到 ~300 KB**：构建期生成紧凑的「字→音节 id」表
      （约 30KB）随词库分发，用 cargo feature 把 `pinyin` crate 从 TIP 里摘掉。
      这个 DLL 被注入到每个宿主进程，体积直接影响宿主启动速度。

0.1.3 已加入中/A 状态图标、系统开关状态同步、全拼/小鹤菜单与持久化设置。横排候选窗采用原生 GDI 绘制，支持圆角、选中态和鼠标点选；Direct2D、暗色主题仍是后续工作。`Ctrl+Space` 已注册为 TSF 保留键，但本机自动化实测未触发切换，仍需排查系统/宿主快捷键分发；图标点击切换已验证。构建期紧凑注音表已替代运行时 `pinyin` 数据，DLL 尚未达到 300 KB 目标。

**已验证**：自建 RichEdit 桌面窗口用真实按键输入 `nihao`，显示「你好」候选，空格上屏、Esc 取消并关闭候选窗。内存 `ITextStoreACP` 测试通过真实 TSF 验证组字、提交、取消、标点和异步写锁。详细记录见 [M1 验证记录](./m1-validation.md)。

**验收矩阵**（下列应用尚未逐项验证，不能视作 M1 已完成）：

| 应用 | 上屏 | 组字串 | 候选窗跟随光标 | 退格/方向键 |
|---|---|---|---|---|
| 记事本 | | | | |
| Word / Excel | | | | |
| Chrome 地址栏 + 网页输入框 | | | | |
| VS Code（Electron） | | | | |
| Windows Terminal | | | | |
| 设置（UWP/WinUI） | | | | |
| 微信/QQ | | | | |

---

## M2 · 上下文与个性化

- [ ] `ContextSnapshot` 采集（TSF `ITfContext` 取光标前后文 + 前台进程信息）
- [ ] 隐私闸门：密码框/黑名单进程 → `PrivacyLevel::None`
- [ ] 用户词库（SQLite）+ `LearningEvent` 异步批量落盘
- [ ] 纠错对采集：上屏后短时间内的手动修改 → `Corrected`
- [ ] 上下文参与本地打分（不依赖网络，纯本地也能变聪明）
- [ ] 候选窗自绘（Win32 + Direct2D，独立 UI 线程，跟随光标，暗色模式）

**验收**：连续两次输入同一串拼音，第二次首选是上次选中的词；在密码框里输入不产生任何上下文日志。

---

## M3 · 二刷（云端）

- [ ] `CloudPinyin` / `LlmReranker` 真实实现（供应商待定，见 ADR-0002）
- [ ] 超时 + 熔断 + `gen` 过期丢弃，全部有单元测试（用 Mock 注入延迟与失败）
- [ ] 二刷「无闪烁刷新」：候选窗只重排不重建
- [ ] `retype-host.exe` 抽取：长连接复用、词库单份、崩溃隔离；`KernelBackend::Remote`
- [ ] 全链路延迟打点（按键→首刷上屏 P50/P95）

**验收**：拔掉网线，打字体验与联网时**完全一致**（只是少热词）；云端服务 hang 死 30s，键盘无任何卡顿。

---

## M4 · 语音输入

- [ ] 音频采集（WASAPI/cpal）20ms 帧 + 有界队列 + 背压
- [ ] `StreamingAsr` 真实实现（WebSocket）
- [ ] `VoiceSession` 三段式状态机：Interim → Stable → Final
- [ ] 松手后 `AudioEnd` + `Optimizing`（"识别优化中"）+ 超时兜底
- [ ] 全局热键（按住说话）
- [ ] 语音上屏的词回写 `UserDict`，影响拼音候选（test.md 图 5 的闭环）

**验收**：说一段 30 秒的话，中途断网 → 已识别部分仍能上屏；松手后 1.5s 内没收到 final → 用 stable 上屏且提示。

---

## M5 · 产品化

- [x] **安装器**：Inno Setup 出 `setup.exe`，负责 HKLM 注册、词库落位、
      Restart Manager 处理「DLL 被所有进程占用」、卸载条目、卸载时询问是否删个人词库
- [ ] **代码签名**：目前没有证书，未签名的 setup.exe 会触发 SmartScreen 警告
      （用户看到「Windows 已保护你的电脑」基本就放弃了）。发布消费级软件前必须解决
- [x] 更新窗口接上安装器：校验通过、用户确认后升级；保存失败/等待重启状态
- [x] 独立版本目录安装，保留正在使用的旧 DLL，不中断旧应用的组字会话
- [x] 登录后及每日计划任务检查，本地 24 小时限频，可关闭/跳过版本
- [ ] 旧版本目录自动回收（目前保留到卸载，避免删除仍被旧应用依赖的文件）
- [ ] Flutter 设置界面（`apps/settings`）：词库管理、热键、隐私白名单、云端开关、日志、检查更新
- [ ] 崩溃上报与延迟打点
- [x] 小鹤双拼：按原始按键构建音节词格，支持部分选词、退格、零声母、全拼切换
- [ ] 其他双拼方案（自然码/微软）
- [ ] Android 端启动：Kotlin IME + JNI → `retype-ffi`；更新逻辑复用 `core/updater`

---

## 里程碑与 test.md 的对应

| 里程碑 | test.md 章节 |
|---|---|
| M0 | 四（首刷）、八（原型 vs 产品的骨架） |
| M1 | 六（Windows 上屏只完成一半） |
| M2 | 三（上下文）、七（个人词库与纠错） |
| M3 | 四（二刷）、九（异常降级） |
| M4 | 一（流式）、二（三段式） |
| M5 | 五（多输入方式共用）、八（产品化） |
