# retype

一个 AI 输入法。**先做 Windows，之后复用同一份内核做 Android。**

架构参考 [`test.md`](./test.md)（豆包输入法的 AI 流水线拆解），完整设计见
**[`ARCHITECTURE.md`](./ARCHITECTURE.md)**。

> 当前进度：**M0（地基）已完成** —— 内核、词库、拼音引擎、TSF 骨架、352k 词的真实词库、
> 首刷延迟达标。还**不能在系统里打中文**（组字串读写是 M1）。详见 [`docs/roadmap.md`](./docs/roadmap.md)。

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
  ffi/                      C ABI（Android JNI 用）
platforms/windows/
  tsf/                      TSF TIP DLL（cdylib）—— 唯一的 unsafe 边界
  candidate-ui/             候选窗呈现接口 + 文本渲染
  diag/                     终端调试台（最重要的开发工具）
  installer/                构建与注册脚本
apps/settings/            Flutter 设置界面
data/dict/                词库源数据（jieba 词频表，MIT）
tools/dict-build/         词库构建：词频表 + 注音 → TSV
docs/                     roadmap · windows-tsf · dict · adr/
```

依赖方向严格单向，`core/*` 里不允许出现 `windows` crate —— 这条规则保证了 Android 端
能直接复用整个内核。

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
| 首刷按键延迟 P50 | **445 µs** | — |
| 首刷按键延迟 P99 | **3.35 ms** | 5 ms ✓ |
| 词库加载 | 0.65 ~ 1.1 s | **必须异步**（`AsyncDict`） |
| 词库构建 | 1.2 s | — |
| TSF 骨架 DLL | 180 KB | 越小越好（注入每个进程） |
| 测试 | 166 项全绿 | — |

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

## 里程碑

| | 目标 | 状态 |
|---|---|---|
| **M0** | 地基：内核 + 拼音引擎 + 真实词库 + TSF 骨架 + 调试台 | ✅ 完成 |
| **M1** | 能在 Windows 里打中文（edit session / 组字串 / 候选窗 / x86+x64） | ⬜ |
| **M2** | 上下文采集 + 用户词库持久化 + 学习闭环 + 候选窗自绘 | ⬜ |
| **M3** | 二刷接真实云端 + 熔断 + host 进程抽取 | ⬜ |
| **M4** | 语音输入：流式 ASR + 三段式 + 热键 | ⬜ |
| **M5** | 安装器 + 设置界面 + 双拼 + Android 端启动 | ⬜ |

M1 的验收是一张**应用兼容性矩阵**（记事本 / Office / Chrome / VS Code / Terminal /
设置 / 微信），见 [docs/roadmap.md](./docs/roadmap.md#m1--能在-windows-里打中文)。

---

## 文档

- [`ARCHITECTURE.md`](./ARCHITECTURE.md) —— 架构总纲，逐节对应 test.md 的七张图
- [`docs/roadmap.md`](./docs/roadmap.md) —— 里程碑与可验证的验收标准
- [`docs/windows-tsf.md`](./docs/windows-tsf.md) —— TSF 实现笔记、已验证事实、稳定性红线
- [`docs/dict.md`](./docs/dict.md) —— 词库管线、二进制格式、打分模型调参
- [`docs/adr/`](./docs/adr) —— 架构决策记录（全 Rust / Mock 先行 / monorepo / 进程内内核）

## 许可与数据来源

代码 MIT。词库原料来自 [jieba](https://github.com/fxsjy/jieba) /
[jieba-rs](https://github.com/messense/jieba-rs)（MIT），署名见
[`data/dict/raw/LICENSE-jieba`](./data/dict/raw/LICENSE-jieba)。
字音数据来自 [`pinyin`](https://crates.io/crates/pinyin) crate（MIT，离线全量）。
