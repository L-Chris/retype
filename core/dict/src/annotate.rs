//! 汉字 → 音节 id 的注音层。
//!
//! 数据来自 [`pinyin`] crate（离线全量字音，含多音字）。
//! 逐字取「首选读音」对多音字词是错的（重庆 ≠ zhong qing），
//! 所以维护一张**词级覆盖表**；覆盖表没命中的多音字词按首选读音处理，
//! 并把词条标上 [`retype_pinyin::flags::AMBIGUOUS`]，让排序更依赖上下文。

use pinyin::{ToPinyin, ToPinyinMulti};
use retype_pinyin::syllables;
use retype_types::SyllableId;
use std::collections::HashMap;
use std::sync::OnceLock;

/// 词级读音覆盖。只收录「逐字首选读音会读错」的常用词，
/// 完整多音词消歧是 M2 上下文打分的工作，不在这里穷举。
pub static OVERRIDES: &[(&str, &str)] = &[
    ("重庆", "chong qing"),
    ("重复", "chong fu"),
    ("重新", "chong xin"),
    ("重要", "zhong yao"),
    ("中国", "zhong guo"),
    ("长大", "zhang da"),
    ("长度", "chang du"),
    ("长篇小说", "chang pian xiao shuo"),
    ("银行", "yin hang"),
    ("行业", "hang ye"),
    ("行走", "xing zou"),
    ("音乐", "yin yue"),
    ("乐曲", "yue qu"),
    ("快乐", "kuai le"),
    ("的确", "di que"),
    ("目的", "mu di"),
    ("首都", "shou du"),
    ("都会", "du hui"),
    ("只有", "zhi you"),
    ("还行", "hai xing"),
    ("还有", "hai you"),
    ("还原", "huan yuan"),
    ("头发", "tou fa"),
    ("发现", "fa xian"),
    ("便宜", "bian yi"),
    ("勉强", "mian qiang"),
    ("倔强", "jue jiang"),
    ("强大", "qiang da"),
    ("数学", "shu xue"),
    ("睡着", "shui zhao"),
    ("看着", "kan zhe"),
    ("好人", "hao ren"),
    ("爱好", "ai hao"),
    ("得到", "de dao"),
    ("心地", "xin di"),
    ("地方", "di fang"),
    ("分量", "fen liang"),
    ("会计", "kuai ji"),
    ("会议", "hui yi"),
    ("参数", "can shu"),
    ("人参", "ren shen"),
    ("身体", "shen ti"),
    ("传单", "chuan dan"),
    ("传达", "chuan da"),
    ("投降", "tou xiang"),
    ("方向", "fang xiang"),
    ("朝向", "chao xiang"),
    ("为难", "wei nan"),
    ("因为", "yin wei"),
    ("成为", "cheng wei"),
    ("重点", "zhong dian"),
    ("重心", "zhong xin"),
    ("调查", "diao cha"),
    ("调整", "tiao zheng"),
    ("空调", "kong tiao"),
    ("空间", "kong jian"),
    ("种子", "zhong zi"),
    ("种植", "zhong zhi"),
    ("种地", "zhong di"),
];

fn override_map() -> &'static HashMap<&'static str, Vec<SyllableId>> {
    static MAP: OnceLock<HashMap<&'static str, Vec<SyllableId>>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for (word, py) in OVERRIDES {
            if let Some(ids) = parse_pinyin(py) {
                m.insert(*word, ids);
            }
        }
        m
    })
}

/// `ü` → `v`，与音节表约定一致（见 `retype_pinyin::syllables`）。
fn norm_syllable(raw: &str) -> String {
    raw.replace('ü', "v").to_ascii_lowercase()
}

/// 解析空格/`'` 分隔的拼音串，如 `"ni hao"`、`"xi'an"`。
pub fn parse_pinyin(s: &str) -> Option<Vec<SyllableId>> {
    let mut ids = Vec::new();
    for part in s.split([' ', '\'', '-']) {
        if part.is_empty() {
            continue;
        }
        ids.push(syllables::id_of(&norm_syllable(part))?);
    }
    if ids.is_empty() {
        None
    } else {
        Some(ids)
    }
}

/// 是否为汉字（CJK 基本区 + 扩展 A）。
pub fn is_han(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

pub fn all_han(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_han)
}

/// 单字的全部读音（多音字会有多个）。
pub fn char_readings(c: char) -> Vec<SyllableId> {
    let mut out = Vec::new();
    if let Some(multi) = c.to_pinyin_multi() {
        for i in 0..multi.count() {
            let n = norm_syllable(multi.get(i).plain());
            if let Some(id) = syllables::id_of(&n) {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
    }
    if out.is_empty() {
        if let Some(p) = c.to_pinyin() {
            let n = norm_syllable(p.plain());
            if let Some(id) = syllables::id_of(&n) {
                out.push(id);
            }
        }
    }
    out
}

/// 首选注音：覆盖表优先，否则逐字取首选读音。非纯汉字返回 `None`。
pub fn annotate(word: &str) -> Option<Vec<SyllableId>> {
    if let Some(ids) = override_map().get(word) {
        return Some(ids.clone());
    }
    if !all_han(word) {
        return None;
    }
    let mut ids = Vec::with_capacity(word.chars().count());
    for c in word.chars() {
        let p = c.to_pinyin()?;
        ids.push(syllables::id_of(&norm_syllable(p.plain()))?);
    }
    Some(ids)
}

/// 该词是否存在多音字歧义（覆盖表未指定，且某个字有多个读音）。
pub fn is_ambiguous(word: &str) -> bool {
    if override_map().contains_key(word) {
        return false;
    }
    word.chars().any(|c| char_readings(c).len() > 1)
}

/// 一个词的全部注音变体。
///
/// - 单字：返回**所有**读音（这样「重」既在 `zhong` 下也在 `chong` 下可查）
/// - 多字：只返回覆盖表读音 + 逐字首选读音。全量笛卡尔积会爆炸，
///   多音词的正确消歧交给上下文打分（M2）。
pub fn annotate_variants(word: &str) -> Vec<Vec<SyllableId>> {
    let mut out: Vec<Vec<SyllableId>> = Vec::new();
    let chars: Vec<char> = word.chars().collect();
    if chars.len() == 1 {
        for id in char_readings(chars[0]) {
            out.push(vec![id]);
        }
        return out;
    }
    if let Some(ids) = override_map().get(word) {
        out.push(ids.clone());
    }
    if let Some(ids) = annotate(word) {
        if !out.contains(&ids) {
            out.push(ids);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn names(ids: &[SyllableId]) -> Vec<&'static str> {
        ids.iter().filter_map(|i| syllables::name_of(*i)).collect()
    }

    #[test]
    fn override_beats_char_by_char() {
        assert_eq!(names(&annotate("重庆").unwrap()), ["chong", "qing"]);
        assert_eq!(names(&annotate("重要").unwrap()), ["zhong", "yao"]);
        assert_eq!(names(&annotate("银行").unwrap()), ["yin", "hang"]);
    }

    #[test]
    fn plain_words_annotate_char_by_char() {
        assert_eq!(names(&annotate("中国").unwrap()), ["zhong", "guo"]);
        assert_eq!(names(&annotate("世界").unwrap()), ["shi", "jie"]);
    }

    #[test]
    fn umlaut_is_normalized_to_v() {
        assert_eq!(names(&annotate("绿").unwrap()), ["lv"]);
        assert_eq!(names(&annotate("女").unwrap()), ["nv"]);
    }

    #[test]
    fn single_char_yields_every_reading() {
        let v = annotate_variants("重");
        let all: Vec<Vec<&'static str>> = v.iter().map(|x| names(x)).collect();
        assert!(all.contains(&vec!["zhong"]), "{all:?}");
        assert!(all.contains(&vec!["chong"]), "{all:?}");
    }

    #[test]
    fn non_han_is_rejected() {
        assert!(annotate("AT&T").is_none());
        assert!(annotate("4S店").is_none());
        assert!(!all_han("你好a"));
        assert!(all_han("你好"));
    }

    #[test]
    fn ambiguity_is_flagged() {
        assert!(
            is_ambiguous("重量"),
            "重 有 zhong/chong 两读且未被覆盖表消歧"
        );
        assert!(!is_ambiguous("重庆"), "覆盖表已消歧，不应再算歧义");
        assert!(!is_ambiguous("世界"), "无多音字");
    }

    #[test]
    fn overrides_have_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for (w, _) in OVERRIDES {
            assert!(seen.insert(*w), "覆盖表里 {w} 重复了");
        }
    }

    #[test]
    fn parse_pinyin_accepts_separators() {
        assert_eq!(names(&parse_pinyin("ni hao").unwrap()), ["ni", "hao"]);
        assert_eq!(names(&parse_pinyin("xi'an").unwrap()), ["xi", "an"]);
        assert!(parse_pinyin("ni haox").is_none());
    }
}
