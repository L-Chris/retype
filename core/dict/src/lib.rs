//! 词库层：系统词库（只读）+ 用户词库（可变）+ 学习回写。
//!
//! 分层见 ARCHITECTURE.md §5；数据管线与二进制格式见 docs/dict.md。
#![forbid(unsafe_code)]

pub mod annotate;
pub mod async_dict;
pub mod learn;
pub mod memory;
pub mod trie;
pub mod user;

pub use async_dict::{spawn_loader, AsyncDict, FallbackPolicy};
pub use learn::{Learner, CORRECTION_CONFIRM_THRESHOLD};
pub use memory::{
    load_annotated, load_word_freq, single_char_fallback, DictBuilder, Entry, LoadOptions,
    LoadStats, MemoryDict,
};
pub use user::{Layer, LayeredDict, UserDict};

use retype_types::SyllableId;
use std::sync::Arc;

/// 用户层在 `LayeredDict` 里的额外加权。
///
/// 默认为 **0**：`UserDict` 里的 logp 已经和系统词库处在同一量级
/// （见 `user::USER_BASE_LOGP` 的注释），再叠一层偏移会让学习曲线失真 ——
/// 用户第一次点某个词它就永久霸占首位，之后再也无法通过选择别的候选来纠正。
/// 保留这个参数只是为了给「激进学习」模式留口子。
pub const DEFAULT_USER_BOOST: f32 = 0.0;

#[derive(Debug, thiserror::Error)]
pub enum DictError {
    #[error("词库 IO 失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("音节不在音节表中: {0}")]
    UnknownSyllable(String),
    #[error("词库文件损坏或为空: {0}")]
    Corrupt(String),
}

/// 组装一个可用的分层词库：系统层 + 用户层。
///
/// `system` 为 `None` 时退化成「单字模式」（ARCHITECTURE.md §7 的降级路径）。
pub fn layered(system: Option<Arc<MemoryDict>>, user: Arc<UserDict>) -> LayeredDict {
    let sys: Arc<dyn retype_pinyin::Lexicon> =
        system.unwrap_or_else(|| Arc::new(single_char_fallback()));
    LayeredDict::with_system_and_user(sys, user, DEFAULT_USER_BOOST)
}

/// 从「词 + 空格分隔拼音」列表快速构建系统词库，主要给测试与 `retype-diag` 用。
///
/// 无法注音的条目会被静默跳过（返回跳过数），因为词库缺几个词不该让输入法起不来。
pub fn from_pairs<'a, I>(pairs: I) -> (MemoryDict, usize)
where
    I: IntoIterator<Item = (&'a str, &'a str, f64)>,
{
    let mut b = DictBuilder::default();
    let mut skipped = 0usize;
    for (text, py, freq) in pairs {
        if b.push_pinyin(text, py, freq).is_err() {
            skipped += 1;
        }
    }
    (b.build(), skipped)
}

/// 音节序列的可读形式，诊断输出用。
pub fn format_syllables(ids: &[SyllableId]) -> String {
    ids.iter()
        .filter_map(|id| retype_pinyin::syllables::name_of(*id))
        .collect::<Vec<_>>()
        .join("'")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use retype_pinyin::{Decoder, Lexicon};

    #[test]
    fn from_pairs_builds_queryable_dict() {
        let (d, skipped) = from_pairs([
            ("你好", "ni hao", 5000.0),
            ("世界", "shi jie", 4000.0),
            ("坏条目", "ni haoo", 1.0),
        ]);
        assert_eq!(skipped, 1);
        assert!(d.has_prefix(&annotate::parse_pinyin("ni hao").unwrap()));
    }

    #[test]
    fn layered_none_system_degrades_to_single_char() {
        let user = Arc::new(UserDict::new());
        let d = layered(None, user);
        // 降级模式下没有任何词，但单字必须能打出来
        let out = Decoder::new().decode("zhongguo", &d);
        assert!(!out.candidates.is_empty());
        assert!(out.candidates.iter().any(|c| c.text.contains('中')));
    }

    #[test]
    fn user_layer_wins_over_system_layer() {
        let (sys, _) = from_pairs([("你好", "ni hao", 100.0), ("拟好", "ni hao", 90.0)]);
        let user = Arc::new(UserDict::new());
        // 12.0 会被 clamp 到 BOOST_CEILING：模拟「用户长期坚定选择」的终态
        user.boost(&annotate::parse_pinyin("ni hao").unwrap(), "拟好", 12.0);
        let d = layered(Some(Arc::new(sys)), Arc::clone(&user));

        let out = Decoder::new().decode("nihao", &d);
        assert_eq!(
            out.candidates.first().map(|c| c.text.as_str()),
            Some("拟好"),
            "用户反复选的词应当顶到首位"
        );
    }

    #[test]
    fn a_single_choice_does_not_hijack_the_top_slot() {
        // 学习曲线必须是渐进的：一次误点不该让某个词永久钉在第一名
        let (sys, _) = from_pairs([("你好", "ni hao", 100000.0), ("拟好", "ni hao", 90.0)]);
        let user = Arc::new(UserDict::new());
        user.boost(&annotate::parse_pinyin("ni hao").unwrap(), "拟好", 0.6);
        let d = layered(Some(Arc::new(sys)), Arc::clone(&user));

        let out = Decoder::new().decode("nihao", &d);
        assert_eq!(
            out.candidates.first().map(|c| c.text.as_str()),
            Some("你好"),
            "高频系统词不该被一次用户选择掀翻"
        );
    }

    #[test]
    fn format_syllables_is_readable() {
        assert_eq!(
            format_syllables(&annotate::parse_pinyin("ni hao ma").unwrap()),
            "ni'hao'ma"
        );
    }
}
