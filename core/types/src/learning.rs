//! 学习事件（ARCHITECTURE.md §5，对应 test.md 图 7）。
//!
//! 三种输入方式共用同一份学习记录，这是 test.md 图 5「共用一个大脑」的落点：
//! 语音上屏过的专业词会影响之后的拼音候选，拼音里纠正过的错词也能喂给语音 final pass。

use crate::event::InputSource;
use crate::SyllableId;

#[derive(Debug, Clone, PartialEq)]
pub enum LearningEvent {
    /// 用户从候选中选了词。记录位置，用于「常选项置顶」。
    CandidateChosen {
        source: InputSource,
        text: String,
        syllables: Vec<SyllableId>,
        /// 在候选列表中的原始位置（0 = 首选，无需学习加权）
        index: usize,
    },
    /// 上屏后短时间内被用户手动改动 → 纠错对（test.md 第七节的核心信号）
    Corrected {
        from: String,
        to: String,
        /// 上下文指纹，避免把无关场景的改动当成通用纠错
        context_hash: u64,
    },
    /// 用户自造词
    Coinage {
        text: String,
        syllables: Vec<SyllableId>,
    },
    /// 语音整段上屏。
    ///
    /// 单独一个变体而不是复用 `CandidateChosen`：语音给的是**自然文本**，
    /// 没有音节序列，需要学习层自己注音后才能进用户词库。
    /// 这正是 test.md 图 5 的闭环 —— 语音学过的专业词要能反哺拼音候选。
    VoiceCommit { text: String },
}

/// 学习记录的持久化后端。M0 提供内存实现，M2 换 SQLite（见 roadmap）。
pub trait LearningStore: Send + Sync {
    fn record(&self, event: LearningEvent);
}
