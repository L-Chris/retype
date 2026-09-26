//! 本地拼音引擎（首刷）。
//!
//! 职责：把用户敲的字母串变成有序候选。全部是纯内存计算，
//! 在输入线程上同步完成，断网也必须工作（ARCHITECTURE.md §2 / test.md 第四节）。
#![forbid(unsafe_code)]

pub mod decode;
pub mod lexicon;
pub mod syllables;

pub use decode::{
    all_segmentations, build_lattice, decode, normalize, DecodeOptions, DecodeOutput, Lattice,
};
pub use lexicon::{flags, EmptyLexicon, LexEntry, Lexicon};

/// 解码门面：归一化 + 词格 + k-best，一次调用拿到候选。
#[derive(Debug, Clone)]
pub struct Decoder {
    pub options: DecodeOptions,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            options: DecodeOptions::default(),
        }
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_options(options: DecodeOptions) -> Self {
        Self { options }
    }

    /// `raw_input` 可以是任意大小写、含 `ü`/`'`；内部会先归一化。
    pub fn decode(&self, raw_input: &str, lex: &dyn Lexicon) -> DecodeOutput {
        let norm = normalize(raw_input);
        decode(&norm, lex, &self.options)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::lexicon::{LexEntry, Lexicon};
    use super::*;
    use retype_types::SyllableId;
    use std::collections::HashMap;

    /// 测试用词库：`("你好", "ni hao", -3.0)` 这种写法，避免在测试里写死音节 id。
    #[derive(Default)]
    struct TestLex {
        map: HashMap<Vec<SyllableId>, Vec<(String, f32)>>,
        prefixes: HashMap<Vec<SyllableId>, ()>,
    }

    impl TestLex {
        fn add(&mut self, text: &str, py: &str, logp: f32) {
            let ids: Vec<SyllableId> = py
                .split_whitespace()
                .map(|s| syllables::id_of(s).unwrap_or(retype_types::SYLLABLE_NONE))
                .collect();
            for i in 1..=ids.len() {
                self.prefixes.insert(ids[..i].to_vec(), ());
            }
            self.map
                .entry(ids)
                .or_default()
                .push((text.to_owned(), logp));
        }
    }

    impl Lexicon for TestLex {
        fn lookup(&self, s: &[SyllableId], out: &mut Vec<LexEntry>) {
            if let Some(v) = self.map.get(s) {
                for (t, l) in v {
                    out.push(LexEntry::new(t.as_str(), *l));
                }
            }
        }
        fn has_prefix(&self, s: &[SyllableId]) -> bool {
            self.prefixes.contains_key(s)
        }
        fn len(&self) -> usize {
            self.map.len()
        }
    }

    fn demo_lex() -> TestLex {
        let mut l = TestLex::default();
        l.add("你", "ni", -3.0);
        l.add("好", "hao", -3.2);
        l.add("你好", "ni hao", -4.0);
        l.add("吗", "ma", -4.5);
        l.add("世", "shi", -4.8);
        l.add("界", "jie", -4.9);
        l.add("世界", "shi jie", -5.2);
        l.add("是", "shi", -2.8);
        l.add("上", "shang", -3.5);
        l.add("海", "hai", -3.9);
        l.add("上海", "shang hai", -5.0);
        l.add("先", "xian", -4.2);
        l.add("西", "xi", -4.4);
        l.add("安", "an", -4.6);
        l.add("西安", "xi an", -6.0);
        l
    }

    #[test]
    fn normalizes_case_and_umlaut() {
        assert_eq!(normalize("NiHao"), "nihao");
        assert_eq!(normalize("lÜ"), "lv");
        assert_eq!(normalize("ni hao"), "nihao");
        assert_eq!(normalize("xi'an"), "xi'an");
    }

    #[test]
    fn segments_ambiguous_input() {
        let segs = all_segmentations("xian", 32);
        assert!(segs.iter().any(|s| s == &["xian"]));
        assert!(segs.iter().any(|s| s == &["xi", "an"]));
    }

    #[test]
    fn separator_forces_boundary() {
        let segs = all_segmentations("xi'an", 32);
        assert!(segs.iter().any(|s| s == &["xi", "an"]));
        assert!(!segs.iter().any(|s| s == &["xian"]));
    }

    #[test]
    fn decodes_full_sentence() {
        let lex = demo_lex();
        let out = Decoder::new().decode("nihaomashijie", &lex);
        assert_eq!(out.syllables, ["ni", "hao", "ma", "shi", "jie"]);
        let texts: Vec<&str> = out.candidates.iter().map(|c| c.text.as_str()).collect();
        assert!(
            texts.iter().any(|t| *t == "你好吗世界"),
            "候选里没有整句: {texts:?}"
        );
    }

    #[test]
    fn word_beats_char_sequence() {
        let lex = demo_lex();
        let out = Decoder::new().decode("nihao", &lex);
        assert_eq!(
            out.candidates.first().map(|c| c.text.as_str()),
            Some("你好"),
            "词频正确的情况下「你好」应排在「你 good」之前"
        );
    }

    #[test]
    fn dictionary_resolves_syllable_ambiguity() {
        let lex = demo_lex();
        // `shanghai` 既能切成 shang+hai 也能切成 shan+g+hai（后者不合法），
        // 关键是必须由词库决定，而不是切分器猜
        let out = Decoder::new().decode("shanghai", &lex);
        assert_eq!(
            out.candidates.first().map(|c| c.text.as_str()),
            Some("上海")
        );
    }

    #[test]
    fn prefix_candidates_let_user_commit_partially() {
        let lex = demo_lex();
        let out = Decoder::new().decode("nihaomashijie", &lex);
        let partial = out
            .candidates
            .iter()
            .find(|c| c.text == "你好")
            .expect("应有前缀候选「你好」");
        assert_eq!(partial.syllable_len, 2, "选中后应推进 2 个音节");
    }

    #[test]
    fn unmatched_letters_are_kept_verbatim() {
        let lex = demo_lex();
        // `q` 不是任何音节的开头，必须原样保留而不是丢弃
        let out = Decoder::new().decode("niq", &lex);
        assert!(out.has_raw);
        assert!(out
            .candidates
            .iter()
            .any(|c| c.text.contains('q') || c.text.contains('你')));
    }

    #[test]
    fn empty_lexicon_degrades_to_raw_letters() {
        // ARCHITECTURE.md §7：词库缺失 → 降级，但绝不能没有输出
        let out = Decoder::new().decode("nihao", &EmptyLexicon);
        assert!(
            !out.candidates.is_empty(),
            "空词库也必须给出候选（原样字母）"
        );
        assert_eq!(out.candidates[0].text, "nihao");
    }

    #[test]
    fn decode_is_deterministic() {
        let lex = demo_lex();
        let d = Decoder::new();
        let a = d.decode("nihaomashijie", &lex);
        let b = d.decode("nihaomashijie", &lex);
        assert_eq!(a.candidates, b.candidates);
    }
}
