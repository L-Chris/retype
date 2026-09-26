//! retype 领域类型。
//!
//! 这一层是整个内核的「词汇表」，**零外部依赖**（只用 std），
//! 平台适配层（Windows TSF / Android IME）与内核之间只通过这里的类型通信。
//! 见 ARCHITECTURE.md §9。
#![forbid(unsafe_code)]

pub mod context;
pub mod event;
pub mod learning;
pub mod render;

pub use context::{AppInfo, ContextSnapshot, FieldInfo, FieldKind, PrivacyLevel};
pub use event::{AsrEvent, InputEvent, InputSource, Key, Modifiers, VoiceEvent};
pub use learning::{LearningEvent, LearningStore};
pub use render::{
    Candidate, CandidateSource, CommitRequest, KernelAction, RenderState, RerankJob, RerankOutcome,
    SideEffect, StatusFlags,
};

/// 代次。每次输入变化 +1，用于丢弃过期的异步结果（ARCHITECTURE.md §2 的 P3 保障）。
pub type Generation = u64;

/// 音节 id，指向 `retype_pinyin::syllables` 里的音节表。
pub type SyllableId = u16;

/// 特殊音节 id：不占用真实音节，用于简拼/通配。
pub const SYLLABLE_NONE: SyllableId = u16::MAX;
