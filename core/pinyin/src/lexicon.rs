//! 词库查询接口。
//!
//! `retype-pinyin` 只**定义**接口，具体实现（系统词库 / 用户词库 / 二进制 mmap）
//! 在 `retype-dict`。这样拼音引擎可以脱离词库实现单独测试，
//! 也让 Android 端能换一套存储而不动解码逻辑。

use retype_types::SyllableId;
use std::sync::Arc;

/// 词条 flags（与 docs/dict.md 的二进制格式保持一致）。
pub mod flags {
    /// 该条目存在多音字歧义，排序时应更依赖上下文
    pub const AMBIGUOUS: u8 = 1 << 0;
    /// 专有名词（人名/地名/机构），云端热词更新时优先
    pub const PROPER: u8 = 1 << 1;
    /// 可被云端热词覆盖
    pub const UPDATABLE: u8 = 1 << 2;
}

/// 一条词库命中。
///
/// `text` 用 `Arc<str>` 而不是 `&'a str`：用户词库是可变的、且藏在锁后面，
/// 借用版本无法把引用安全地交出去。Arc clone 只是一次原子自增，
/// 相比一次 String 分配便宜得多，同时彻底消除了跨层的生命周期传染。
#[derive(Debug, Clone)]
pub struct LexEntry {
    pub text: Arc<str>,
    /// 归一化对数概率，越大越常见（通常为负）
    pub logp: f32,
    pub flags: u8,
}

impl LexEntry {
    pub fn new(text: impl Into<Arc<str>>, logp: f32) -> Self {
        Self {
            text: text.into(),
            logp,
            flags: 0,
        }
    }

    pub fn with_flags(mut self, flags: u8) -> Self {
        self.flags = flags;
        self
    }
}

/// 词库抽象。所有方法都必须是**纯内存查询**，绝不允许 IO（ARCHITECTURE.md P1）。
pub trait Lexicon: Send + Sync {
    /// 精确查询：给定音节 id 序列，追加所有命中到 `out`。
    ///
    /// 实现方**只 push，不要 clear**，buffer 由调用方复用以减少分配。
    fn lookup(&self, syllables: &[SyllableId], out: &mut Vec<LexEntry>);

    /// 前缀剪枝：是否存在以 `syllables` 开头的词条。
    ///
    /// 解码器靠它把词格扩展从「所有音节组合」降到「词库里真实存在的前缀」，
    /// 是首刷延迟的关键。默认返回 `true`（不剪枝，正确但慢）。
    fn has_prefix(&self, _syllables: &[SyllableId]) -> bool {
        true
    }

    /// 简拼：按声母序列查询（如 `b,j,d,x` → 北京大学）。M1 实现，默认无结果。
    fn lookup_initials(&self, initials: &[u8], _out: &mut Vec<LexEntry>) {
        let _ = initials;
    }

    /// 词条总数。0 → 触发单字模式降级（ARCHITECTURE.md §7）。
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 空词库。用于触发降级路径的测试（ARCHITECTURE.md §7）。
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyLexicon;

impl Lexicon for EmptyLexicon {
    fn lookup(&self, _syllables: &[SyllableId], _out: &mut Vec<LexEntry>) {}
    fn has_prefix(&self, _syllables: &[SyllableId]) -> bool {
        false
    }
    fn len(&self) -> usize {
        0
    }
}
