//! 渲染状态、上屏请求与内核副作用。

use crate::context::ContextSnapshot;
use crate::event::InputSource;
use crate::{Generation, SyllableId};

/// 候选来源。会显示在候选窗上（云端项可加标记），也参与合并规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CandidateSource {
    /// 本地词库（首刷）
    Local,
    /// 解码兜底路径中的原样字母；保留给诊断，不展示为中文候选
    Raw,
    /// 单字降级
    SingleChar,
    /// 用户词库 / 个人常用
    User,
    /// 云端整句或重排结果（二刷）
    Cloud,
    /// 云端热词
    Hotword,
}

/// 一条候选。
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// 上屏文字
    pub text: String,
    /// 展示用拼音注释，如 `ni'hao`
    pub comment: String,
    pub source: CandidateSource,
    /// 这条候选消费了几个音节
    pub syllable_len: usize,
    /// 这条候选消费了几个**输入字符**。
    ///
    /// 和 `syllable_len` 不是一回事：`xi'an` 用 5 个字符表达 2 个音节，
    /// `zhong` 用 5 个字符表达 1 个音节。内核靠它精确切走已消费的组字串，
    /// 少一个字符就会让用户看到残留字母。
    pub consumed: usize,
    /// 对应的音节 id 序列（学习回写、上下文匹配用）
    pub syllables: Vec<SyllableId>,
    /// 打分，越大越靠前。仅用于诊断，排序以 `candidates` 顺序为准。
    pub score: f32,
}

impl Candidate {
    pub fn new(text: impl Into<String>, source: CandidateSource) -> Self {
        Self {
            text: text.into(),
            comment: String::new(),
            source,
            syllable_len: 0,
            consumed: 0,
            syllables: Vec::new(),
            score: 0.0,
        }
    }
}

/// 状态位。手写位掩码，避免为一个类型引入 bitflags。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct StatusFlags(u16);

impl StatusFlags {
    pub const EMPTY: Self = Self(0);
    /// 中文模式（否则英文直通）
    pub const CHINESE: Self = Self(1 << 0);
    pub const FULL_WIDTH_PUNCT: Self = Self(1 << 1);
    /// 云端可用（断网/熔断时清零）
    pub const CLOUD_OK: Self = Self(1 << 2);
    /// 二刷进行中
    pub const CLOUD_BUSY: Self = Self(1 << 3);
    pub const VOICE_RECORDING: Self = Self(1 << 4);
    /// 「识别优化中」（test.md 第二节第三阶段）
    pub const VOICE_OPTIMIZING: Self = Self(1 << 5);
    /// 处于降级状态（词库缺失 / host 不可用 / 云端熔断）
    pub const DEGRADED: Self = Self(1 << 6);

    #[inline]
    pub const fn bits(self) -> u16 {
        self.0
    }
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    #[inline]
    pub const fn with(self, other: Self) -> bool {
        Self(self.0 | other.0).contains(other)
    }
    #[inline]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    #[inline]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

/// 内核 → 平台层的渲染状态。
///
/// `gen` 是防「候选乱跳」的关键：异步二刷结果必须带着发出时的 gen 回来，
/// 内核发现 gen 落后就整包丢弃（ARCHITECTURE.md §2）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RenderState {
    pub gen: Generation,
    /// 组字串全文 = 已转换部分 + 未转换的拼音字母
    pub composition: String,
    /// `composition` 中已转换（已选词）部分的**字符数**。
    /// TSF 用它划分 display attribute：前面是「已转换」，后面是「未转换」下划线。
    pub converted_len: usize,
    /// 未转换部分切分出的音节，用于显示 `ni'hao'ma`
    pub syllables: Vec<String>,
    pub candidates: Vec<Candidate>,
    /// 当前高亮的候选下标（绝对下标，不是页内下标）
    pub selected: usize,
    /// 每页候选数（候选窗翻页用）
    pub page_size: usize,
    /// 当前页起始下标
    pub page_start: usize,
    pub status: StatusFlags,
}

impl RenderState {
    pub fn is_empty(&self) -> bool {
        self.composition.is_empty() && self.candidates.is_empty()
    }

    /// 当前页可见的候选。
    pub fn visible(&self) -> &[Candidate] {
        let end = (self.page_start + self.page_size.max(1)).min(self.candidates.len());
        let start = self.page_start.min(end);
        &self.candidates[start..end]
    }

    pub fn page_count(&self) -> usize {
        let size = self.page_size.max(1);
        self.candidates.len().div_ceil(size)
    }
}

/// 上屏请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitRequest {
    /// 直接插入
    Text(String),
    /// 替换当前组字串（TSF 里就是 composition range 的 SetText + EndComposition）
    ReplaceComposition { text: String },
}

/// 一次二刷任务。由内核产生，平台/后端在 worker 线程执行。
#[derive(Debug, Clone, PartialEq)]
pub struct RerankJob {
    pub gen: Generation,
    pub source: InputSource,
    /// 原始拼音串
    pub composition: String,
    pub syllables: Vec<String>,
    /// 首刷候选（二刷只能重排 + 追加，不许删除，见 P3）
    pub candidates: Vec<Candidate>,
    /// 已按隐私盖章；云端实现还会再调 `sanitized_for_cloud`
    pub context: ContextSnapshot,
}

/// 二刷结果。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RerankOutcome {
    /// 重排后的候选（可能与首刷部分重叠）
    pub ranked: Vec<Candidate>,
    /// 云端新增：整句候选、热词
    pub extra: Vec<Candidate>,
    /// true 表示这次没拿到有效云端结果（超时/失败/被熔断），内核据此更新状态位
    pub degraded: bool,
}

/// 内核要求平台层执行的异步副作用。**绝不在输入线程上执行。**
#[derive(Debug, Clone, PartialEq)]
pub enum SideEffect {
    Rerank(RerankJob),
    Learn(crate::learning::LearningEvent),
    /// 请求采集一次上下文（TSF 侧异步读 ITfContext）
    CollectContext,
}

/// 内核对外的动作。平台适配层只需要处理这四种。
#[derive(Debug, Clone, PartialEq)]
pub enum KernelAction {
    Render(RenderState),
    Commit(CommitRequest),
    Side(SideEffect),
    /// 按键不由输入法处理，交回宿主（英文直通、快捷键等）
    PassThrough,
}
