//! 云端能力抽象层。
//!
//! 见 [ADR-0002](../../../docs/adr/0002-cloud-abstraction-mock-first.md)：
//! M0–M2 只用 Mock，但 trait 形状按真实供应商设计，替换时不动上层。
//!
//! 三个契约贯穿全模块：
//! 1. **只承诺语义，不承诺传输**。`AsrEvent` 用 test.md 的三段式命名（Interim/Stable/Final），
//!    这个划分是供应商无关的。
//! 2. **云端只能重排 + 追加**。`RerankResponse::order` 是本地候选的下标序列，
//!    类型层面就不允许云端删掉用户已经看到的候选（P3）。
//! 3. **出网前必须过隐私闸门**。`CloudClient` 会强制调用
//!    [`ContextSnapshot::sanitized_for_cloud`]，实现方无需（也不应）自己判断。

pub mod breaker;
pub mod client;
pub mod mock;

pub use breaker::{BreakerState, CircuitBreaker};
pub use client::{CloudClient, CloudResult};
pub use mock::{MockAsr, MockCloudPinyin, MockConfig, MockLlmReranker, UnavailableCloud};

use futures::future::BoxFuture;
use retype_types::{AsrEvent, Candidate, ContextSnapshot, InputSource};

/// 云端错误。所有变体都必须能跨线程传递（`Clone + Send`）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CloudError {
    #[error("网络错误: {0}")]
    Network(String),
    #[error("鉴权失败: {0}")]
    Auth(String),
    #[error("请求超时")]
    Timeout,
    #[error("服务不可用")]
    Unavailable,
    #[error("熔断器已打开")]
    CircuitOpen,
    #[error("响应格式非法: {0}")]
    Malformed(String),
}

/// 云拼音请求（test.md 图 4 的「二刷」输入）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PinyinRequest {
    /// 完整拼音串，如 `nihaomashijie`
    pub composition: String,
    /// 已切分的音节，如 `["ni","hao","ma","shi","jie"]`
    pub syllables: Vec<String>,
    /// 已按隐私闸门处理的上下文
    pub context: ContextSnapshot,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PinyinSuggestion {
    /// 云端整句候选
    pub sentences: Vec<String>,
    /// 热词
    pub hotwords: Vec<String>,
}

/// 重排请求。`candidates` 是首刷结果，云端只能改顺序、加内容。
#[derive(Debug, Clone, PartialEq)]
pub struct RerankRequest {
    pub composition: String,
    pub syllables: Vec<String>,
    pub candidates: Vec<Candidate>,
    pub context: ContextSnapshot,
    pub source: InputSource,
}

/// 重排响应。
///
/// `order` 用**下标**而不是文字：这样即使云端返回了本地不存在的词，
/// 也只能进 `extra`，无法冒充本地候选，合并逻辑（P3）因此可以完全信任它。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RerankResponse {
    /// `candidates` 的重排结果，元素是下标。缺失的下标由内核补回末尾。
    pub order: Vec<usize>,
    /// 云端新增：整句候选、热词
    pub extra: Vec<Candidate>,
    /// LLM 润色后的整句（可选）
    pub polished: Option<String>,
}

pub trait CloudPinyin: Send + Sync {
    /// 生成整句候选与热词。
    ///
    /// 实现必须自带内部超时；`CloudClient` 的外层超时只是最后一道保险。
    fn suggest(
        &self,
        req: PinyinRequest,
    ) -> BoxFuture<'static, Result<PinyinSuggestion, CloudError>>;
}

pub trait LlmReranker: Send + Sync {
    /// 根据上下文对本地候选重排。
    fn rerank(&self, req: RerankRequest) -> BoxFuture<'static, Result<RerankResponse, CloudError>>;

    /// 语音 final pass 的整段润色（test.md 第二节第三阶段）。
    /// 默认原样返回，未接入 LLM 时不影响主流程。
    fn polish(
        &self,
        text: String,
        _context: ContextSnapshot,
    ) -> BoxFuture<'static, Result<String, CloudError>> {
        Box::pin(async move { Ok(text) })
    }
}

/// ASR 配置。
#[derive(Debug, Clone, PartialEq)]
pub struct AsrConfig {
    pub sample_rate: u32,
    pub language: String,
    /// 热词/上下文，用于提升专有名词命中率
    pub context: ContextSnapshot,
    /// 用户纠错对（test.md 第七节：历史纠正反哺语音识别）
    pub corrections: Vec<(String, String)>,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            language: "zh-CN".into(),
            context: ContextSnapshot::empty(),
            corrections: Vec::new(),
        }
    }
}

/// 一次语音会话。
///
/// 刻意做成**同步 push + 非阻塞 poll** 而不是 async stream：
/// 音频采集线程需要「塞进去就走」，绝不能在采集线程上等 future
/// （ARCHITECTURE.md §3：采集/编码/发送/接收必须并发）。
pub trait AsrSession: Send {
    /// 送入一帧编码后的音频。队列满时应丢弃最旧帧而不是阻塞调用方。
    fn push_audio(&mut self, frame: &[u8]) -> Result<(), CloudError>;
    /// 音频发送完毕，请求 final pass。**松手时必须调用**，
    /// 否则最后一次整段校正会丢失（test.md 第二节）。
    fn finish(&mut self) -> Result<(), CloudError>;
    /// 非阻塞取出已到达的事件。
    fn poll(&mut self) -> Vec<AsrEvent>;
    /// final 是否已到达（或已确定不会到达）。
    fn is_done(&self) -> bool;
    /// 主动取消并断开。
    fn cancel(&mut self);
}

pub trait StreamingAsr: Send + Sync {
    fn start(&self, cfg: AsrConfig) -> Result<Box<dyn AsrSession>, CloudError>;
}
