# retype

一个 AI 输入法。**先做 Windows，之后复用同一份内核做 Android。**

架构参考 [`test.md`](./test.md)（豆包输入法的 AI 流水线拆解），完整设计见
**[`ARCHITECTURE.md`](./ARCHITECTURE.md)**。

> 当前进度：**M1 桌面输入预览** —— 已接通 TSF 组字、中文上屏、候选窗和中英切换，
> 独立 RichEdit 窗口实测 `nihao` → `你好`。包含 x86/x64 DLL 和二进制词库。
> 主流应用兼容性矩阵尚未完成，现代应用支持仍待验证。详见 [`docs/roadmap.md`](./docs/roadmap.md)。

0.1.3 新增语言栏「中 / A」状态图标，点击切换中英；右键菜单选择全拼或小鹤双拼，选择会保存。
全拼输入 `nihao`，小鹤输入 `nihc`，空格选择「你好」。候选窗采用圆角横排，每页五项，支持数字和鼠标选词。

---

## 一句话架构

> 一个平台无关的「统一输入内核」（Rust），被各平台的薄适配层包住；
> 内核内部严格区分「同步首刷」与「异步二刷」，任何跨网络的调用都不允许出现在按键处理路径上。

```text
Windows: TSF TIP DLL     Android: InputMethodService     ← 薄适配层，只做翻译/绘制/上屏
        └────────────┬───────────────┘
              retype-engine  统一输入与联想内核            ← 语音/键盘/触摸共用一个大脑
        ┌──────┬─────┴────┬─────────┬──────────┐
     pinyin  dict      context    cloud      voice
     本地引擎 词库/学习  上下文   ASR/LLM    流式识别
```

三条不可违背的原则（来自 test.md 第六节）：

| | 原则 | 工程约束 |
|---|---|---|
| **P1** | 输入主线程绝不阻塞 | TSF 回调内禁止网络、文件 IO、词典加载、跨调用持锁 |
| **P2** | 失败必须优雅降级 | 每个外部依赖都有本地兜底路径（[降级矩阵](./ARCHITECTURE.md#7-降级矩阵)） |
| **P3** | 已展示的内容不许消失 | 二刷只能重排 + 追加；语音 final 不能丢掉 stable 段 |

---

## 为什么不是 Flutter

仓库原本是 Flutter 脚手架，但 **Flutter 无法实现 Windows 输入法**：Windows IME 必须是
TSF（Text Services Framework）的 COM in-proc DLL，由系统注入到每个应用进程；
Flutter Windows 只能产出独立 `.exe`。Android 同理，必须是 `InputMethodService`。

所以：**内核与平台层全部用 Rust**（[ADR-0001](./docs/adr/0001-full-rust-core-and-tsf.md)），
Flutter 迁到 [`apps/settings`](./apps/settings) 只做设置界面（[ADR-0003](./docs/adr/0003-monorepo-layout.md)）。
TSF 用纯 Rust 实现是可行的，已验证（[docs/windows-tsf.md](./docs/windows-tsf.md#已验证事实)）。

---

## 目录

```text
core/                     跨平台内核（Rust，不依赖任何平台 API）
  types/                    领域类型：InputEvent / RenderState / Candidate / CommitRequest
  pinyin/                   音节表 · 切分 · 词格 · k-best Viterbi（首刷）
  dict/                     系统词库 · 用户词库 · 分层加权 · 学习回写 · 异步加载
  context/                  上下文采集接口 + 隐私闸门
  cloud/                    CloudPinyin / LlmReranker / StreamingAsr + Mock + 熔断器
  engine/                   统一内核：状态机、首刷/二刷编排、三段式语音、去抖后端
  updater/                  自动更新核心：semver 比较 · release 解析 · sha256 校验
  ffi/                      C ABI（Android JNI 用）
platforms/windows/
  tsf/                      TSF TIP DLL（cdylib）—— 唯一的 unsafe 边界
  candidate-ui/             候选窗呈现接口 + 文本渲染
  diag/                     终端调试台（最重要的开发工具）
  updater/                  retype-updater.exe（独立进程，TIP 绝不做网络 IO）
  installer/                build.ps1 · package.ps1 · register.ps1
apps/settings/            Flutter 设置界面
data/dict/                词库源数据（jieba 词频表，MIT）
tools/dict-build/         词库构建：词频表 + 注音 → TSV
tools/scripts/            fetch-jieba-dict.ps1
docs/                     roadmap · windows-tsf · dict · auto-update · adr/ · releases/
.github/workflows/        ci.yml（PR/main）· release.yml（tag v* → 构建并发布）
```

依赖方向严格单向，`core/*` 里不允许出现 `windows` crate —— CI 的 `core-portability`
job 在 **ubuntu** 上编译全部内核 crate 来强制这条规则，保证 Android 端能直接复用内核。

---

## 快速开始

前置：Rust stable（`x86_64-pc-windows-msvc`）、VS 2022 Build Tools（C++ 工作负载）。

```powershell
# 1. 跑测试（166 项）
cargo test --workspace

# 2. 构建词库（349k 词 → 352,357 条已注音词条，约 1.2s，产物 8.7MB）
cargo run -p retype-dict-build --release -- `
    --in  data/dict/raw/jieba-dict.txt `
    --out data/dict/retype-dict.tsv

# 3. 在终端里试打字（不需要注册 TSF，最快的开发回路）
cargo run -p retype-diag --release -- --dict data/dict/retype-dict.tsv

# 4. 首刷延迟基准
cargo run -p retype-diag --release -- --bench --dict data/dict/retype-dict.tsv

# 5. 排查「为什么 A 排在 B 前面」
cargo run -p retype-diag --release -- --dict data/dict/retype-dict.tsv --explain nihaomashijie
```

或者一把梭：`platforms\windows\installer\build.ps1`（构建 + 词库 + 自检 + 基准 + 收集到 `dist\windows`）。

Flutter 设置界面：`cd apps/settings; flutter run -d windows`

---

## `retype-diag`：为什么它是核心工具

调 TSF 最痛的是「改一行 → 重新注册 → 注销重登 → 打开记事本试」。
`retype-diag` 把内核从 TSF 里剥出来跑在终端上，**跑的是同一个内核、同一份词库、
同一条首刷/二刷链路**，只是把候选窗换成文本渲染、把 TSF 事件换成 stdin。
90% 的迭代应该在这里完成。

```text
拼音> nihaomashijie
[42] 中 · 云
  组字: ni'hao'ma'shi'jie
  候选:
   ▶ 1. 你好吗世界  [ ]  <ni'hao'ma'shi'jie>
     2. 你好吗     [ ]  <ni'hao'ma>
     ...
拼音> :ctx 这是一段光标前文      ← 设置上下文，观察二刷重排
拼音> :voice 今天天气不错        ← 模拟一次完整语音三段式
拼音> 1                          ← 选第 1 个候选
★ 上屏(替换组字串): 你好吗世界
```

`:help` 看全部命令。

---

## 实测数据（release / x86_64 / 352,357 词条）

| 指标 | 实测 | 预算 |
|---|---|---|
| 首刷按键延迟 P50 | **293 µs** | — |
| 首刷按键延迟 P99 | **2.22 ms** | 5 ms ✓ |
| 词库加载 | 0.65 ~ 1.1 s | **必须异步**（`AsyncDict`） |
| 词库构建 | 1.2 s | — |
| TSF TIP DLL | **767.5 KiB（x64）/ 652 KiB（x86），0.1.3** | ~300 KB，尚未达标 |
| 安装包 setup.exe | ~8 MiB（含双架构和二进制词库） | — |
| 测试 | 全工作区与 x86 TSF 测试，见 [验证记录](./docs/m1-validation.md) | 2 项安装注册检查默认跳过 |

> M1 已将 `pinyin` 移到构建依赖，运行时只使用生成的紧凑注音表，DLL 从 M0 的约 1,082 KB 缩小。
> 300 KB 目标尚未达到。上述延迟为本地内核基准，不包含宿主 TSF 调度和候选窗绘制。

---

## 已经踩过的坑（都写进了文档，别重复踩）

- **`windows` 0.62 没有 `implement` feature**，但 `#[implement]` 要求把 `windows-core`
  加为直接依赖，否则报 `cannot find windows_core in the crate root`。
- feature 名是 `Win32_UI_Input_Ime`，**不是** `Win32_UI_Input_Methods`（不存在）。
  写错时 cargo 会把几万个 feature 全打印出来，注意重定向输出。
- `AdviseKeyEventSink` 在 **`ITfKeystrokeMgr`** 上，不在 `ITfThreadMgr` 上（要先 QI）。
- `windows_core::BOOL`，不是 `Win32::Foundation::BOOL`（0.62）。
- `Param<T, InterfaceType>` 只接受**借用**：`AdviseKeyEventSink(tid, &sink, true)`。
- `pinyin` crate 的 `plain()` 返回 **`lü`**（带分音符），必须归一成 `lv` 才能对上音节表。
- **k-best 不能用「(位置, 槽位下标)」回溯** —— top-k 是边扩展边插入/截断的，下标会失效，
  拼出词库里根本不存在的组合。已改用 `Rc` 链表（有回归测试）。
- **unigram 打分有 `−k·ln T` 偏差**：词频≈3 的垃圾词条「妳好吗」会压过「你好 + 吗」。
  解法是每词**加分** `word_bonus`，实测取 3.5。完整扫描表见
  [docs/dict.md](./docs/dict.md#打分模型unigram-的固有偏差与-word_bonus)。

---

## 版本号、CI 与发布

**版本号只有两处**，必须一致（CI 的 `version-consistency` job 会挡）：

- `Cargo.toml` 的 `[workspace.package] version` ← 唯一真源，所有 crate 继承
- `apps/settings/pubspec.yaml` 的 `version:`（写成 `0.1.0+1`，构建号在后）

当前版本：**0.1.0**

| workflow | 触发 | 做什么 |
|---|---|---|
| [`ci.yml`](./.github/workflows/ci.yml) | push main / PR | 内核 crate 在 **ubuntu** 编译（跨平台铁律）· Windows 全量 fmt/clippy/test · 词库构建 · 延迟基准（P99 超 5ms 直接失败）· Flutter analyze/test · 版本号一致性 |
| [`release.yml`](./.github/workflows/release.yml) | push tag `v*.*.*` | 校验 tag==Cargo==pubspec · 质量门 · x64/x86 构建与 TSF 测试 · 词库 · 基准 · **Inno Setup 打 setup.exe + sha256** · 发布 GitHub Release |

发版流程：

```powershell
# 1. 改版本号（Cargo.toml + pubspec.yaml）
# 2. 写发布说明（缺了这个文件 release 会直接失败，不会发一个空说明的版本）
#    docs/releases/v0.1.1.md
git tag v0.1.1
git push origin v0.1.1
```

产物命名必须与 `core/updater` 的 `Platform::asset_suffix()` 对齐
（`retype-<版本>-windows-x64-setup.exe` + 同名 `.sha256`），否则更新器挑不到安装包。
CI 里有一步专门断言这两边一致，改一边忘另一边会直接红。

## 自动更新

参考 `torto-app` 的机制（查 GitHub `releases/latest` → 比对版本 → 给出 release 页），
但针对输入法做了三处改动，完整设计见 **[`docs/auto-update.md`](./docs/auto-update.md)**：

1. **更新逻辑不在 TIP DLL 里**，而是独立的 `retype-updater.exe`。
   TIP 被注入到每个宿主进程，在里面发 HTTP 等于让 Chrome/Word 替我们打流量，
   而且直接违反 P1。
2. **完整 semver 比较**（含预发布优先级）。参考实现按点切分整数比，
   对 `0.2.0-rc.1` 会得出错误结论。
3. **sha256 校验通过才落盘**，且校验文件必须与产物成对存在 ——
   自动更新等于「从网上下载一个会被注入到每个进程的 DLL」，校验不是可选项。
4. 发的是 **Inno Setup 安装包**而不是 zip：输入法要写 HKLM 的 CTF 注册表键、
   要把 8.9MB 词库放进 Program Files、还要处理「DLL 正被所有进程占用」。
   每次安装进入独立版本目录，再切换注册路径；已打开的应用继续使用旧 DLL。
   升级不关闭应用、不自动重启，同版本修复也分配新目录。

```powershell
retype-updater.exe check                       # 退出码 0=已最新 10=有更新 2=网络错误
retype-updater.exe check --json                # 给设置界面用
retype-updater.exe download --out <目录>        # 下载 + 校验，通过后才写盘
retype-updater.exe update                      # 打开更新窗口，确认后下载并安装
retype-updater.exe update --background         # 自动检查，有新版本才提醒
```

仓库地址解析优先级：`--repo` > 环境变量 `RETYPE_GITHUB_REPO` > 编译期烘入值
（CI 用 `github.repository` 注入，所以发布的 exe 天然知道自己的仓库；
开发构建没这个变量会明确报错，不会悄悄打到占位地址）。

语言栏右键和开始菜单都提供更新入口。当前用户的计划任务在登录后、每天检查，
本地限频为 24 小时；可在更新窗口关闭自动检查或跳过版本。后台检查不会自动安装。
安装后校验版本、双架构 DLL 哈希、COM 路径和输入法名称。旧应用需重新打开以加载新版。
从旧安装器迁移时，如果仍有待重启替换任务，必须先完成该次重启。

---

## 里程碑

| | 目标 | 状态 |
|---|---|---|
| **M0** | 地基：内核 + 拼音引擎 + 真实词库 + TSF 骨架 + 调试台 | ✅ 完成 |
| **M1** | 能在 Windows 里打中文（edit session / 组字串 / 候选窗 / x86+x64） | 预览可用，兼容性验收中 |
| **M2** | 上下文采集 + 用户词库持久化 + 学习闭环 + 候选窗自绘 | ⬜ |
| **M3** | 二刷接真实云端 + 熔断 + host 进程抽取 | ⬜ |
| **M4** | 语音输入：流式 ASR + 三段式 + 热键 | ⬜ |
| **M5** | 安装器（原子替换 DLL）+ 设置界面 + 双拼 + Android 端启动 | ⬜ |

M1 的验收是一张**应用兼容性矩阵**（记事本 / Office / Chrome / VS Code / Terminal /
设置 / 微信），见 [docs/roadmap.md](./docs/roadmap.md#m1--能在-windows-里打中文)。

---

## 文档

- [`ARCHITECTURE.md`](./ARCHITECTURE.md) —— 架构总纲，逐节对应 test.md 的七张图
- [`docs/roadmap.md`](./docs/roadmap.md) —— 里程碑与可验证的验收标准
- [`docs/windows-tsf.md`](./docs/windows-tsf.md) —— TSF 实现笔记、已验证事实、稳定性红线
- [`docs/dict.md`](./docs/dict.md) —— 词库管线、二进制格式、打分模型调参
- [`docs/auto-update.md`](./docs/auto-update.md) —— 自动更新设计与信任边界
- [`docs/adr/`](./docs/adr) —— 架构决策记录（全 Rust / Mock 先行 / monorepo / 进程内内核）
- [`docs/releases/`](./docs/releases) —— 每个 tag 的发布说明（release.yml 强制要求存在）

## 许可与数据来源

代码 MIT。词库原料来自 [jieba](https://github.com/fxsjy/jieba) /
[jieba-rs](https://github.com/messense/jieba-rs)（MIT），署名见
[`data/dict/raw/LICENSE-jieba`](./data/dict/raw/LICENSE-jieba)。
字音数据来自 [`pinyin`](https://crates.io/crates/pinyin) crate（MIT，离线全量）。
