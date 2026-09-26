//! 音节表：整个项目里「什么是一个合法拼音音节」的**唯一真源**。
//!
//! `tools/dict-build` 与运行时共用这张表，避免两边音节 id 不一致。
//! 约定：`ü` 一律写成 `v`（用户键盘上就是敲 `lv` / `nv`）。

use retype_types::SyllableId;
use std::collections::HashMap;
use std::sync::OnceLock;

/// 全部合法音节（不带声调），按声母分组便于人工核对。
/// id 就是本数组下标，因此**只能追加，不能在中间插入**（否则已构建的 dict.bin 会错位）。
pub static SYLLABLES: &[&str] = &[
    // 零声母
    "a", "ai", "an", "ang", "ao", "e", "ei", "en", "eng", "er", "o", "ou", // b
    "ba", "bai", "ban", "bang", "bao", "bei", "ben", "beng", "bi", "bian", "biao", "bie", "bin",
    "bing", "bo", "bu", // p
    "pa", "pai", "pan", "pang", "pao", "pei", "pen", "peng", "pi", "pian", "piao", "pie", "pin",
    "ping", "po", "pou", "pu", // m
    "ma", "mai", "man", "mang", "mao", "me", "mei", "men", "meng", "mi", "mian", "miao", "mie",
    "min", "ming", "miu", "mo", "mou", "mu", // f
    "fa", "fan", "fang", "fei", "fen", "feng", "fo", "fou", "fu", // d
    "da", "dai", "dan", "dang", "dao", "de", "dei", "den", "deng", "di", "dia", "dian", "diang",
    "diao", "die", "ding", "diu", "dong", "dou", "du", "duan", "dui", "dun", "duo", // t
    "ta", "tai", "tan", "tang", "tao", "te", "teng", "ti", "tian", "tiao", "tie", "ting", "tong",
    "tou", "tu", "tuan", "tui", "tun", "tuo", // n
    "na", "nai", "nan", "nang", "nao", "ne", "nei", "nen", "neng", "ni", "nian", "niang", "niao",
    "nie", "nin", "ning", "niu", "nong", "nou", "nu", "nuan", "nuo", "nv", "nve", // l
    "la", "lai", "lan", "lang", "lao", "le", "lei", "leng", "li", "lia", "lian", "liang", "liao",
    "lie", "lin", "ling", "liu", "lo", "long", "lou", "lu", "luan", "lun", "luo", "lv", "lve",
    // g
    "ga", "gai", "gan", "gang", "gao", "ge", "gei", "gen", "geng", "gong", "gou", "gu", "gua",
    "guai", "guan", "guang", "gui", "gun", "guo", // k
    "ka", "kai", "kan", "kang", "kao", "ke", "ken", "keng", "kong", "kou", "ku", "kua", "kuai",
    "kuan", "kuang", "kui", "kun", "kuo", // h
    "ha", "hai", "han", "hang", "hao", "he", "hei", "hen", "heng", "hong", "hou", "hu", "hua",
    "huai", "huan", "huang", "hui", "hun", "huo", // j
    "ji", "jia", "jian", "jiang", "jiao", "jie", "jin", "jing", "jiong", "jiu", "ju", "juan",
    "jue", "jun", // q
    "qi", "qia", "qian", "qiang", "qiao", "qie", "qin", "qing", "qiong", "qiu", "qu", "quan",
    "que", "qun", // x
    "xi", "xia", "xian", "xiang", "xiao", "xie", "xin", "xing", "xiong", "xiu", "xu", "xuan",
    "xue", "xun", // zh
    "zha", "zhai", "zhan", "zhang", "zhao", "zhe", "zhei", "zhen", "zheng", "zhi", "zhong", "zhou",
    "zhu", "zhua", "zhuai", "zhuan", "zhuang", "zhui", "zhun", "zhuo", // ch
    "cha", "chai", "chan", "chang", "chao", "che", "chen", "cheng", "chi", "chong", "chou", "chu",
    "chua", "chuai", "chuan", "chuang", "chui", "chun", "chuo", // sh
    "sha", "shai", "shan", "shang", "shao", "she", "shei", "shen", "sheng", "shi", "shou", "shu",
    "shua", "shuai", "shuan", "shuang", "shui", "shun", "shuo", // r
    "ran", "rang", "rao", "re", "ren", "reng", "ri", "rong", "rou", "ru", "ruan", "rui", "run",
    "ruo", // z
    "za", "zai", "zan", "zang", "zao", "ze", "zei", "zen", "zeng", "zi", "zong", "zou", "zu",
    "zuan", "zui", "zun", "zuo", // c
    "ca", "cai", "can", "cang", "cao", "ce", "cen", "ceng", "ci", "cong", "cou", "cu", "cuan",
    "cui", "cun", "cuo", // s
    "sa", "sai", "san", "sang", "sao", "se", "sen", "seng", "si", "song", "sou", "su", "suan",
    "sui", "sun", "suo", // y
    "ya", "yan", "yang", "yao", "ye", "yi", "yin", "ying", "yo", "yong", "you", "yu", "yuan",
    "yue", "yun", // w
    "wa", "wai", "wan", "wang", "wei", "wen", "weng", "wo", "wu",
    // ── 罕用 / 语气音节 ─────────────────────────────────────────────
    // 必须**追加在末尾**：音节 id == 数组下标，中间插入会让已构建的 dict.bin 全部错位。
    "ng", "hm", "hng", "kei", "tei", "len", "nun", "nia", "rua", "fiao",
];

/// 刻意**不收录**的音节，及原因。
///
/// - `m` / `n`：单字母音节会和几乎所有音节产生切分歧义（`an` → `a`+`n`），
///   而对应的字（呣/嗯）已经能通过 `hm` / `ng` 打出来，收益远低于代价。
/// - `ê`：不是 ASCII，键盘上打不出来，`normalize` 会直接丢弃。
pub static EXCLUDED: &[&str] = &["m", "n", "ê"];

/// 最长音节长度（`zhuang` / `chuang` / `shuang`）。
pub const MAX_SYLLABLE_LEN: usize = 6;

/// 声母表，用于简拼（首字母匹配）与模糊音。
pub const INITIALS: &[&str] = &[
    "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x", "zh", "ch", "sh", "r",
    "z", "c", "s", "y", "w",
];

fn index() -> &'static HashMap<&'static str, SyllableId> {
    static INDEX: OnceLock<HashMap<&'static str, SyllableId>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut m = HashMap::with_capacity(SYLLABLES.len() * 2);
        for (i, s) in SYLLABLES.iter().enumerate() {
            // 只保留首个，重复项说明表写错了（有单元测试守着）
            m.entry(*s).or_insert(i as SyllableId);
        }
        m
    })
}

/// 音节字符串 → id。
#[inline]
pub fn id_of(syllable: &str) -> Option<SyllableId> {
    index().get(syllable).copied()
}

/// id → 音节字符串。
#[inline]
pub fn name_of(id: SyllableId) -> Option<&'static str> {
    SYLLABLES.get(id as usize).copied()
}

/// 音节的声母（首字母），简拼用。零声母返回首字母本身（如 `a` → `a`）。
pub fn initial_of(id: SyllableId) -> Option<u8> {
    name_of(id).and_then(|s| s.as_bytes().first().copied())
}

/// 音节数量，诊断用。
pub fn count() -> usize {
    SYLLABLES.len()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::collections::HashSet;

    #[test]
    fn no_duplicate_syllables() {
        let set: HashSet<&str> = SYLLABLES.iter().copied().collect();
        assert_eq!(set.len(), SYLLABLES.len(), "音节表里有重复项");
    }

    #[test]
    fn ids_round_trip() {
        for (i, s) in SYLLABLES.iter().enumerate() {
            assert_eq!(id_of(s), Some(i as SyllableId));
            assert_eq!(name_of(i as SyllableId), Some(*s));
        }
    }

    #[test]
    fn covers_core_ambiguity_cases() {
        for s in [
            "xian", "jian", "shang", "hai", "shan", "chuan", "zhuang", "lv", "nv", "er", "an",
        ] {
            assert!(id_of(s).is_some(), "缺少音节 {s}");
        }
        // 这两个是「词」不是「音节」，切分歧义应由词库解决（见 docs/dict.md）
        assert!(id_of("shanghai").is_none());
        assert!(id_of("changan").is_none());
    }

    #[test]
    fn excluded_syllables_are_intentional() {
        for s in EXCLUDED {
            assert!(id_of(s).is_none(), "{s} 应当被排除，见 EXCLUDED 注释");
        }
    }

    /// 用 `pinyin` crate 的全量字音反查，确保音节表没有漏项。
    /// crate 返回 `lü` 这类带分音符的形式，需先归一成 `lv`。
    #[test]
    fn covers_every_syllable_in_pinyin_crate() {
        use pinyin::ToPinyinMulti;
        let mut missing: Vec<String> = Vec::new();
        for cp in 0x3400u32..0x9FFF {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            let Some(multi) = c.to_pinyin_multi() else {
                continue;
            };
            for i in 0..multi.count() {
                let raw = multi.get(i).plain();
                let norm = raw.replace('ü', "v");
                if EXCLUDED.contains(&norm.as_str()) {
                    continue;
                }
                if id_of(&norm).is_none() && !missing.contains(&norm) {
                    missing.push(norm);
                }
            }
        }
        assert!(missing.is_empty(), "音节表缺项: {missing:?}");
    }
}
