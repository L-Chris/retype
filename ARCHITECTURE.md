# retype 架构设计

> 参考架构：[`test.md`](./test.md)（豆包输入法的 AI 流水线拆解）。
> 本文把 test.md 的产品级描述，落成 retype 的**工程结构、模块边界、线程模型与降级策略**。
> 首期目标平台：**Windows（TSF）**；Android 复用同一份内核，见 [roadmap](./docs/roadmap.md)。

---

## 0. 一句话架构

> **一个平台无关的「统一输入内核」（Rust），被各平台的薄适配层包住；内核内部严格区分「同步首刷」与「异步二刷」，任何跨进程/跨网络的调用都不允许出现在按键处理路径上。**

```text
┌───────────────────── 平台适配层（薄） ─────────────────────┐
│  Windows: TSF TIP DLL      Android: InputMethodService     │
│  · 按键/焦点/组字/上屏      · 按键/触摸/组字/上屏           │
│  · 候选窗（Direct2D）       · 候选窗（Compose）             │
│  · 上下文采集（TSF/UIA）    · 上下文采集（EditorInfo）      │
└───────────────────────────┬───────────────────────────────┘
                            │  统一抽象：InputEvent / RenderState / CommitRequest
┌───────────────────────────▼───────────────────────────────┐
│              retype-engine  统一输入与联想内核              │
│  会话状态机 · 候选合并 · 首刷/二刷编排 · 降级 · 学习回写     │
├──────────┬──────────┬──────────┬──────────┬───────────────┤
│ pinyin   │ dict     │ context  │ cloud    │ voice(后期)   │
│ 本地引擎 │ 词库/学习│ 上下文   │ ASR/LLM  │ 流式识别      │
└──────────┴──────────┴──────────┴──────────┴───────────────┘
```

对应 test.md **图 5**（语音、键盘、触摸共用一个「大脑」）：三种输入方式只是 `InputEvent` 的不同生产者，内核以下的所有能力（上下文、候选、词库、学习、上屏）完全共享。

---

## 1. 三条不可违背的原则

来自 test.md 第六节，落成硬性约束（CI 会检查其中可自动化的部分）：

| # | 原则 | 工程约束 |
|---|---|---|
| P1 | **输入主线程绝不阻塞** | TSF 回调内禁止：网络、文件 IO、`Mutex` 跨调用持有、词典加载、`async` block_on。只允许读内存态 + 投递任务。 |
| P2 | **失败必须优雅降级** | 每个外部依赖都有「本地兜底路径」，见 [§7 降级矩阵](#7-降级矩阵)。 |
| P3 | **已展示的内容不许消失** | 二刷只能「重排 + 追加」，不能删掉用户已经看到/正在选的候选；语音 final 不能丢掉 stable 段。 |

> P3 是 test.md 反复强调的点：*"即使 AI 二次刷新超时，用户已经看到的候选也不能消失。"*

---

## 2. 数据流：拼音输入（首刷 + 二刷）

对应 test.md **图 4**（本地拼音与云端 AI 协作）。

```text
 按键 'n','i','h','a','o'
      │  (TSF 主线程, 同步, 预算 < 1ms)
      ▼
 KeyEventSource ──InputEvent::Key──► retype-engine
                                        │
                    ┌───────────────────┴───────────────────┐
                    │ 同步首刷 (必须在本帧内出结果)          │
                    ▼                                       │
             pinyin::decode(buffer)                         │
             ├ 音节切分 (DP over 音节表)                     │
             ├ 词格构建 (系统词库 + 用户词库 + 上下文加权)    │
             └ Viterbi k-best ──► CandidateList(v1, gen=1)  │
                    │                                       │
                    ▼                                       │
             RenderState ──► 候选窗（独立 UI 线程）          │
                    │                                       │
                    │  ┌────────────────────────────────────┘
                    │  │ 异步二刷 (worker 线程 / 后期 host 进程)
                    ▼  ▼
             RerankJob { pinyin, candidates, ContextSnapshot, gen=1 }
                    │
                    ▼
             cloud::CloudPinyin + LlmReranker   ← 超时 / 熔断 / Mock
                    │
             ┌──────┴───────┐
             │ gen 过期?     │─是─► 丢弃（P3：绝不用旧结果覆盖新状态）
             └──────┬───────┘
                    │否
                    ▼
             merge(v1, v2)：保留 v1 全部条目，重排 + 追加云端整句/热词
                    │
                    ▼
             RenderState' ──► 候选窗「无闪烁刷新」
```

**关键设计**：`gen`（generation）单调递增，每次按键 +1。二刷结果携带发出时的 `gen`，回来时若 `gen` 已落后则直接丢弃 —— 这是防止「输入快了之后候选乱跳」的唯一可靠手段。

**合并规则**（P3 的具体实现）：

```text
merged = []
for c in v2.ranked:            # 云端/AI 排序
    if c in v1: merged.push(c)          # 只允许提升本地已有项
    else if allow_cloud_insert: merged.push(c)   # 整句/热词，标记来源
for c in v1:
    if c not in merged: merged.push(c)  # 本地候选一个都不许丢，兜底追加
```

**去抖（debounce）是二刷的前提，不是优化**。用户连续打字时每次按键都会产生一个新任务，
而前面那些**必然过期**（gen 已落后）。与其发出去再丢弃，不如在 `LocalBackend` 的
worker 里等一个停顿：窗口内（默认 120ms）不断有新任务就用最新的替换旧的，只在真正
停手时发一次请求。否则积压的请求会在停手后一起回来，把候选搅乱 —— 这正是 test.md
说的「在不打断输入的情况下刷新候选」的工程含义。

---

## 3. 数据流：语音输入（三段式）

对应 test.md **图 1 / 图 2**。本轮只落接口与状态机，不接真实 ASR（见 [ADR-0002](./docs/adr/0002-cloud-abstraction-mock-first.md)）。

```text
        ┌──────────── VoiceSession 状态机（retype-engine）────────────┐
        │                                                             │
 Idle ──按下热键──► Recording ──首帧音频──► Streaming                  │
        │              │                       │                      │
        │              │                 Interim(text) ──► 组字预览    │
        │              │                       │        （临时，可变） │
        │              │                 Stable(text)  ──► 稳定段      │
        │              │              （停顿触发 two-pass，前文被改写） │
        │              │                       │                      │
        │        松开热键                       │                      │
        │              ▼                       ▼                      │
        └──────► Finalizing ──send AudioEnd──► Optimizing ──► Done    │
                                  │           （"识别优化中"）  │      │
                                  │                            ▼      │
                              timeout(默认 1.5s)          CommitRequest│
                                  │                                   │
                                  └──► 用最后一个 Stable 兜底上屏      │
        └─────────────────────────────────────────────────────────────┘
```

三条硬要求（都来自 test.md）：

1. **松手不等于关连接**。必须发 `AudioEnd` 并等待 final pass，否则丢掉最后一次整段校正。
2. **超时也要上屏**。`Optimizing` 超过阈值就用最后的 `Stable` 提交，不能让用户的字消失。
3. **采集/发送/接收/UI 四者并发**。任何一个环节串行化都会导致「开头几个字丢失」。

音频链路（后期）：

```text
麦克风 ─► 采集线程(WASAPI/cpal, 20ms 帧) ─► 有界队列 ─► 编码(PCM16/Opus)
                                                          │
                                              WebSocket 发送任务
                                                          │
                                              接收任务 ─► AsrEvent ─► 状态机
```
有界队列 + 背压：满了丢**最旧**的未编码帧并打点，绝不阻塞采集线程。

---

## 4. 上下文（准确率的真正来源）

对应 test.md **图 3** / 第三节。

```rust
// 数据模型在 core/types/src/context.rs（零依赖，Android 直接复用）
// 采集接口与隐私策略在 core/context/src/lib.rs（采集实现在平台层）
pub struct ContextSnapshot {
    pub app: AppInfo,              // 进程名/包名、窗口标题、控件类型（聊天/搜索/代码/文档）
    pub field: FieldInfo,          // 输入框能力：是否可读上下文、是否密码框、最大长度
    pub text_before: String,       // 光标前 N 字（默认 64，可配）
    pub text_after: String,        // 光标后 M 字（默认 16）
    pub selection: Option<String>, // 当前选中文字（替换语义）
    pub privacy: PrivacyLevel,     // None / Local / Cloud —— 决定这份上下文能不能出网
}
```

**隐私闸门是架构的一部分，不是附加项**：

| PrivacyLevel | 允许行为 |
|---|---|
| `None` | 密码框、用户显式禁止的进程 —— 上下文为空，不采集 |
| `Local` | 只用于本地词库加权重排，**绝不出网** |
| `Cloud` | 可随请求发给云端 ASR/LLM |

`PrivacyLevel` 由 `context` 模块判定并**盖章**在快照上；`cloud` 模块在发请求前必须校验，`privacy != Cloud` 时静默剥离上下文字段。这样「上下文泄漏」在类型层面就不可能发生。

**采集失败 = 空上下文，不是错误**（P2）。TSF 的 `ITfContext` 在某些应用里会拒绝读取（如部分游戏、UWP 沙箱），必须 `Result` → `None` 静默降级。

---

## 5. 词库与个性化学习

对应 test.md **图 7** / 第七节。

```text
候选打分 = f( 系统词频 , 用户词权重 , 上下文相关性 , 时间衰减 , 纠错对 )

分层（从下到上，越上层越个性化，覆盖优先级越高）：
  ┌────────────────────────────────────┐
  │ L4 CorrectionPairs  原始→用户改后   │  ← test.md 第七节的核心信号
  ├────────────────────────────────────┤
  │ L3 UserDict         自造词/常用词   │  SQLite，异步批量写
  ├────────────────────────────────────┤
  │ L2 RecentHistory    最近 N 次上屏   │  环形缓冲，内存
  ├────────────────────────────────────┤
  │ L1 SystemDict       词频词库(只读)  │  mmap 二进制，进程内共享
  └────────────────────────────────────┘
```

**学习事件**（全部异步落盘，永不阻塞输入线程）：

```rust
pub enum LearningEvent {
    /// 用户从候选中选了词 —— 记录位置，用于「常选项置顶」
    CandidateChosen { source: InputSource, raw: String, chosen: String, index: u16 },
    /// 用户上屏后手动改了字 —— 生成纠错对（原始 → 修改后）
    Corrected { from: String, to: String, context_hash: u64 },
    /// 用户自造词
    Coinage { word: String, pinyin: String },
}
```

**跨输入方式共享**（test.md 图 5 的落点）：语音上屏过的专业词 → 进 `UserDict` → 影响之后的拼音候选；拼音里纠正过的错词 → 进 `CorrectionPairs` → 提供给语音 final pass 做后处理。因为 L1–L4 都在 `retype-dict`，与输入方式无关，这件事是**免费**的。

词库数据管线见 [docs/dict.md](./docs/dict.md)。

---

## 6. 线程与进程模型

对应 test.md **图 6**。

```text
┌──────────────────────── 宿主应用进程（notepad / chrome / office…）────────────────────────┐
│                                                                                          │
│  ┌─ TSF 主线程（宿主应用的 UI 线程）────────────────────────┐                             │
│  │  ITfKeyEventSink::OnKeyDown / OnTestKeyDown               │  ← P1：零阻塞              │
│  │  ITfEditSession（组字串读写）                             │                             │
│  │  ITfThreadFocusSink（焦点变化）                           │                             │
│  │  内存态读取 + crossbeam 投递任务，立即返回                 │                             │
│  └───────────────┬──────────────────────────┬───────────────┘                             │
│                  │ 任务投递                  │ RenderState（无锁队列）                     │
│                  ▼                          ▼                                             │
│  ┌─ 内核 worker 线程 ──────────┐  ┌─ 候选窗 UI 线程 ───────────┐                        │
│  │ retype-rerank: 二刷（去抖）  │  │ 独立消息循环 + Direct2D     │                        │
│  │ retype-learn : 学习落盘      │  │ 分层窗口，跟随光标          │                        │
│  │ retype-dict-load: 词库加载   │  │                            │                        │
│  │ （MVP 在进程内，见 ADR-0004）│  └────────────────────────────┘                        │
│  └───────────────┬──────────────┘                                                      │
└──────────────────┼────────────────────────────────────────────────────────────────────────┘
                   │ （M3 起可选：抽到 retype-host.exe，命名管道 + 共享内存）
                   ▼
        ┌─ retype-host.exe（常驻，单例）─────────────┐
        │ 长连接复用（WS/HTTP2）· 音频设备持有        │
        │ 系统词库单份内存映射 · 用户词库单写者       │
        │ 崩溃不影响输入法打字（P2）                  │
        └────────────────────────────────────────────┘
```

**为什么 MVP 用进程内线程、后期抽 host 进程？**

TIP DLL 被注入到**每一个**应用进程里。如果每个进程都持有一份系统词库和一条 WS 长连接，内存和连接数会随打开的应用数线性膨胀。但一开始就上 IPC，会让「打字延迟」这个最难调的问题变得几乎无法归因。

所以：**先用进程内线程把延迟和正确性调对，把 IPC 边界抽象成 trait**，等内核稳定后再把 `KernelBackend` 的实现从 `Local` 换成 `Remote`，上层零改动。详见 [ADR-0004](./docs/adr/0004-in-proc-kernel-first.md)。

```rust
// core/engine/src/backend.rs
pub trait KernelBackend: Send + Sync {
    fn submit(&self, ev: InputEvent) -> Vec<KernelAction>; // 只含首刷，按键 P99 ≤ 6ms
    fn poll_action(&self) -> Option<KernelAction>;         // 非阻塞取异步结果
    fn render(&self) -> RenderState;                       // 当前快照
}
// LocalBackend:  同进程 worker 线程（二刷去抖 + 学习落盘）
// InlineBackend: 同步执行副作用，只给测试和 retype-diag 用
// RemoteBackend: 命名管道 + 共享内存环（M3）
```

`KernelBackend` 刻意**不带泛型方法**，否则就不是 dyn-compatible 的，
`Arc<dyn KernelBackend>` 用不了 —— 详见 §9。

---

## 7. 降级矩阵

来自 test.md 第六/九节 —— *"一款输入法是否稳定，取决于异常发生时它能不能优雅降级。"*

| 故障 | 检测 | 降级行为 | 用户可感知 |
|---|---|---|---|
| 断网 | 请求失败/DNS | 纯本地拼音，二刷静默跳过 | 候选少了热词，**打字不受影响** |
| 云端超时 | 单次超时（默认 800ms） | 保留首刷结果，丢弃迟到响应 | 无 |
| 云端持续失败 | 熔断器（连续 5 次） | 30s 内不再发起二刷 | 无 |
| 上下文读取被拒 | TSF 返回错误 | 空上下文，`privacy=None` | 排序略差 |
| 系统词库缺失/损坏 | 加载校验失败 | 退化为**单字模式**（`pinyin` crate 全量字音） | 只能打单字，但仍可打字 |
| 用户词库损坏 | SQLite 打开失败 | 隔离坏文件（改名 `.corrupt`）后重建 | 个人词丢失，不崩 |
| host 进程崩溃 | 管道断开 | 自动切回 `Local` 后端 | 短暂无热词 |
| 候选窗创建失败 | HWND 为空 | 走 TSF 原生候选 UI（`ITfCandidateListUIElement`） | 样式变丑，功能在 |
| ASR final 未返回 | `Optimizing` 超时 | 用最后一个 `Stable` 上屏 | 少了整段润色 |
| 内核 panic | `catch_unwind` 包裹 | 当次按键透传（英文原样上屏），打点上报 | 一次输入没生效，**应用不崩** |

最后一行尤其重要：TIP 运行在**别人的进程里**，我们 panic 就是让 Chrome 崩溃。所有 TSF 回调入口都必须 `catch_unwind`，且 Rust 侧禁止 `unwrap()`（clippy `unwrap_used = deny`）。

---

## 8. 模块与依赖

```text
core/
  types/       retype-types      领域类型，零依赖（InputEvent/RenderState/Candidate/…）
  pinyin/      retype-pinyin     音节表 · 切分 · 词格 · Viterbi k-best · 模糊音/简拼
  dict/        retype-dict       SystemDict(mmap) · UserDict · 分层加权 · 学习 · AsyncDict(异步加载)
  context/     retype-context    ContextSnapshot 模型 + 隐私闸门（采集实现在平台层）
  cloud/       retype-cloud      CloudPinyin/LlmReranker/StreamingAsr traits + Mock + 熔断
  engine/      retype-engine     统一内核：会话状态机、首刷/二刷编排、合并、降级、KernelBackend
  learning/    retype-learning-store  跨平台 SQLite 学习存储与同步数据格式
  updater/     retype-updater    自动更新：semver 比较、release 解析、sha256 校验（HTTP 抽象成 trait）
  ffi/         retype-ffi        C ABI 导出（Android JNI / 外部诊断）

platforms/windows/
  tsf/           retype-tsf          TSF TIP DLL（cdylib）—— 只做 TSF 管线，不含业务逻辑
  candidate-ui/  retype-candidate-ui 候选窗呈现接口（Win32+Direct2D 自绘在 M2）
  diag/          retype-diag         终端调试台：敲拼音看候选，最快的开发回路
  updater/       retype-updater      独立更新器 exe（TIP DLL 绝不做网络 IO）
  installer/                         build.ps1 / package.ps1 / register.ps1

platforms/android/               Kotlin IME + Compose 键盘／设置 + retype-android JNI 会话
apps/settings-egui/              Windows 设置界面
apps/settings/                   早期 Flutter 设置原型（未用于 Android 预览）
tools/dict-build/                词库构建：词频表 + 拼音 → dict.bin
data/dict/                       词库源数据
```

依赖方向严格单向（CI 用「内核 crate 在 ubuntu 上编译」这条 job 强制检查）：

```text
types ◄── pinyin ◄── dict ◄── engine ──► tsf / ffi
   ▲                    ▲        ▲  ▲
   └── context ─────────┘        │  └── candidate-ui
   └── cloud ────────────────────┘

updater（独立，只依赖 serde_json/sha2）──► platforms/windows/updater
```

**铁律**：`core/*` 里不允许出现 `windows` crate（用 `#[cfg(windows)]` 隔离的平台胶水除外，且只能放在 `platforms/`）。这条规则由 CI 的 `core-portability` job 在 **ubuntu** 上编译全部内核 crate 来强制 —— 一旦有人在 `core/` 里引了平台 API，那个 job 会先红。

同理 `core/updater` 不含任何网络实现，HTTP 走 `HttpFetcher` trait，真实实现（`ureq`）只在 `platforms/windows/updater` 里。详见 [docs/auto-update.md](./docs/auto-update.md)。

---

## 9. 关键接口

```rust
// ---- core/types ----
pub enum InputSource { Keyboard, Voice, Touch }

pub enum InputEvent {
    Key { key: Key, mods: Modifiers, source: InputSource },
    ToggleChinese,
    FocusChanged { app: AppInfo, field: FieldInfo },
    ContextUpdated(ContextSnapshot),
    Voice(VoiceEvent),                 // Start / Stop / Cancel / OptimizeTimeout / Asr(..)
    CandidateChosen { index: usize },
    CandidatePage { delta: i32 },
    RerankCompleted { gen: Generation, result: RerankOutcome },  // 二刷回来了，带发出时的 gen
}

pub struct RenderState {
    pub gen: Generation,               // 代次，防止旧结果覆盖新状态
    pub composition: String,           // 组字串全文 = 已转换 + 未转换的拼音
    pub converted_len: usize,          // 已转换部分的字符数（TSF display attribute 的分界）
    pub syllables: Vec<String>,        // 未转换部分的音节切分，用于显示 ni'hao'ma
    pub candidates: Vec<Candidate>,    // 已按 §2 合并规则排好
    pub selected: usize,
    pub page_size: usize,
    pub page_start: usize,
    pub status: StatusFlags,           // 中英/全半角/云端可用/录音中/识别优化中/降级
}

pub struct Candidate {
    pub text: String,
    pub comment: String,               // 拼音提示
    pub source: CandidateSource,       // Local | SingleChar | User | Cloud | Hotword
    pub syllable_len: usize,           // 消费了几个音节
    pub consumed: usize,               // 消费了几个输入字符（≠音节数：xi'an 是 5 字符 2 音节）
    pub syllables: Vec<SyllableId>,
    pub score: f32,
}

pub enum CommitRequest { Text(String), ReplaceComposition { text: String } }

pub enum KernelAction { Render(RenderState), Commit(CommitRequest), Side(SideEffect), PassThrough }
pub enum SideEffect  { Rerank(RerankJob), Learn(LearningEvent), CollectContext }

// ---- core/cloud（全部可 Mock，见 ADR-0002）----
pub trait CloudPinyin: Send + Sync {
    fn suggest(&self, req: PinyinRequest) -> BoxFuture<'static, Result<PinyinSuggestion, CloudError>>;
}
pub trait LlmReranker: Send + Sync {
    fn rerank(&self, req: RerankRequest) -> BoxFuture<'static, Result<RerankResponse, CloudError>>;
    fn polish(&self, text: String, ctx: ContextSnapshot) -> BoxFuture<'static, Result<String, CloudError>>;
}
pub trait StreamingAsr: Send + Sync {
    fn start(&self, cfg: AsrConfig) -> Result<Box<dyn AsrSession>, CloudError>;
}
```

`RerankResponse::order` 刻意是**下标序列**而不是候选列表：这样云端在类型层面就只能
「重排本地候选 + 通过 `extra` 追加」，无法删掉用户已经看到的候选，也无法伪造一个
「本地候选」。P3 因此不依赖调用方自觉。

平台适配层只需要三件事：**把系统事件翻译成 `InputEvent`**、**把 `RenderState` 画出来**、
**把 `CommitRequest` 写回输入框**。这就是 Windows 和 Android 能共用内核的全部原因。

后端契约（见 [ADR-0004](./docs/adr/0004-in-proc-kernel-first.md)）：

```rust
pub trait KernelBackend: Send + Sync {
    /// 同步处理事件，返回必须立即执行的动作。耗时只含首刷（按键 P99 ≤ 6ms）
    fn submit(&self, ev: InputEvent) -> Vec<KernelAction>;
    /// 非阻塞取出异步产生的动作（二刷完成后的重渲染）
    fn poll_action(&self) -> Option<KernelAction>;
    /// 当前渲染状态快照
    fn render(&self) -> RenderState;
}
```

注意这里**不能**用 `with_kernel<R>(impl FnOnce(&Kernel) -> R)`：带泛型方法的 trait
不是 dyn-compatible 的，`Arc<dyn KernelBackend>` 就没法用了，而「平台层只依赖 trait
object」正是 M3 能无痛换成 `RemoteBackend` 的前提。需要直接摸内核时，用具体类型
（`LocalBackend` / `InlineBackend`）上的同名固有方法。


---

## 10. 与 test.md 的对应关系

| test.md 章节 | 本架构落点 |
|---|---|
| 一、流式识别不是「录完再识别」 | §3 音频链路：采集/编码/发送/接收/UI 五者并发，有界队列 + 背压 |
| 二、文字为什么反复变化（三段式） | §3 `VoiceSession` 状态机：Interim / Stable / Final |
| 三、上下文才是准确率差距 | §4 `ContextSnapshot` + `PrivacyLevel` 闸门 |
| 四、拼音候选不只在本地算（首刷/二刷） | §2 同步首刷 + 异步二刷 + `gen` 代次 + 合并规则 |
| 五、多种输入方式共用一个大脑 | §0/§8 `retype-engine` 唯一内核，`InputSource` 只是标签 |
| 六、Windows 上屏只完成一半（拆模块） | §6 线程/进程模型 + P1 零阻塞约束 |
| 七、个人词库与纠错记录 | §5 L1–L4 分层 + `LearningEvent` |
| 八、不只是接入一个模型 | §7 降级矩阵（这才是产品与原型差距所在） |
| 九、异常降级决定能否长期使用 | §7 + P2/P3 |

---

## 11. 待决问题

- **候选窗渲染**：Direct2D 自绘 vs 复用 TSF 原生 `ITfCandidateListUIElement`。MVP 先用原生（省事、兼容性由系统保证），M2 换自绘以拿到豆包那种视觉。
- **简拼/双拼**：MVP 只做全拼 + 简拼，双拼（自然码/小鹤/微软）在 M4 以「键位映射层」形式插入，不改内核。
- **词库授权**：`data/dict/raw/` 使用 jieba 词频表（MIT），需保留署名，见 [docs/dict.md](./docs/dict.md)。
- **Android 端 UI（已决定）**：键盘和设置均用 Jetpack Compose，JNI 复用 Rust 内核与共享学习存储；生命周期、编辑器权限和上屏确认由 Kotlin 适配层负责，详见 [Android](docs/android.md)。
