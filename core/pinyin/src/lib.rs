//! 本地拼音引擎（首刷）。
//!
//! 职责：把用户敲的字母串变成有序候选。全部是纯内存计算，
//! 在输入线程上同步完成，断网也必须工作（ARCHITECTURE.md §2 / test.md 第四节）。
#![forbid(unsafe_code)]

pub mod decode;
pub mod lexicon;
pub mod syllables;

pub use decode::{
    all_segmentations, build_lattice, decode, normalize, trace, DecodeOptions, DecodeOutput,
    Lattice, PathStep, PathTrace,
};
pub use lexicon::{flags, EmptyLexicon, LexEntry, Lexicon};

/// 解码门面：归一化 + 词格 + k-best，一次调用拿到候选。
#[derive(Debug, Clone, Default)]
pub struct Decoder {
    pub options: DecodeOptions,
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
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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

    /// 测试词表：既是 `demo_lex` 的数据源，也是「候选必须由这些词拼成」的断言依据。
    /// 两处共用一份，避免断言列表漏词导致假失败。
    const DEMO_WORDS: &[(&str, &str, f32)] = &[
        ("你", "ni", -3.0),
        ("好", "hao", -3.2),
        ("你好", "ni hao", -4.0),
        ("吗", "ma", -4.5),
        ("世", "shi", -4.8),
        ("界", "jie", -4.9),
        ("世界", "shi jie", -5.2),
        ("是", "shi", -2.8),
        ("上", "shang", -3.5),
        ("海", "hai", -3.9),
        ("上海", "shang hai", -5.0),
        ("先", "xian", -4.2),
        ("西", "xi", -4.4),
        ("安", "an", -4.6),
        ("西安", "xi an", -6.0),
    ];

    fn demo_lex() -> TestLex {
        let mut l = TestLex::default();
        for (w, py, logp) in DEMO_WORDS {
            l.add(w, py, *logp);
        }
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
        assert!(texts.contains(&"你好吗世界"), "候选里没有整句: {texts:?}");
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

    /// 回归测试：k-best 曾用「(位置, 槽位下标)」回溯，而每层 top-k 是边扩展边
    /// 插入/截断的，槽位下标会失效，于是拼出词库里根本不存在的组合
    /// （真实词库上的表现是「妳好吗世界」压过「你好吗世界」）。
    #[test]
    fn every_candidate_is_a_valid_word_sequence() {
        let lex = demo_lex();
        // 长词优先匹配，否则「你好」会被切成「你」+「好」而误判
        let mut allowed: Vec<&str> = DEMO_WORDS.iter().map(|w| w.0).collect();
        allowed.sort_by_key(|w| std::cmp::Reverse(w.len()));
        for input in ["nihaomashijie", "nihaonihao", "shijienihaoma", "xianshijie"] {
            let out = Decoder::new().decode(input, &lex);
            assert!(!out.candidates.is_empty(), "{input} 没有候选");
            for c in &out.candidates {
                let mut rest = c.text.as_str();
                while !rest.is_empty() {
                    let Some(w) = allowed.iter().find(|w| rest.starts_with(**w)) else {
                        panic!("{input} 的候选 {:?} 含词库外片段: {rest:?}", c.text);
                    };
                    rest = &rest[w.len()..];
                }
            }
        }
    }

    /// 首选路径必须覆盖整串输入且不掺原样字母。
    ///
    /// 这里刻意**不**断言具体是哪个词排第一：合成词库的词频不构成真实语言模型，
    /// 排序会随 `word_bonus` 调整而变（真实词库上的调参结果见 docs/dict.md）。
    /// 真正要守的不变量是「首选是一条完整、干净的切分」—— k-best 回溯出 bug 时
    /// 第一个被破坏的就是它。
    #[test]
    fn best_path_covers_the_whole_input() {
        let lex = demo_lex();
        for (input, syllables) in [
            ("nihaomashijie", 5usize),
            ("nihao", 2),
            ("shanghai", 2),
            ("xianshijie", 3),
        ] {
            let out = Decoder::new().decode(input, &lex);
            let first = out.candidates.first().unwrap_or_else(|| {
                panic!("{input} 没有候选");
            });
            assert!(!out.has_raw, "{input} 的首选掺了原样字母: {:?}", first.text);
            assert_eq!(
                first.syllable_len, syllables,
                "{input} 的首选应覆盖全部 {syllables} 个音节，实际 {:?} / {}",
                first.text, first.syllable_len
            );
            assert_eq!(
                first.consumed,
                input.len(),
                "{input} 的首选应消费全部输入字符"
            );
        }
    }

    #[test]
    fn full_sentence_reading_is_among_candidates() {
        let lex = demo_lex();
        let out = Decoder::new().decode("nihaomashijie", &lex);
        let texts: Vec<&str> = out.candidates.iter().map(|c| c.text.as_str()).collect();
        assert!(
            texts.contains(&"你好吗世界"),
            "整句读法应在候选里: {texts:?}"
        );
    }

    #[test]
    fn long_input_stays_within_latency_budget() {
        // P1 的量化形式：本地首刷必须远小于一帧。
        // 这里用一个明显超长的输入压一下词格规模，确认没有指数爆炸。
        let lex = demo_lex();
        let input = "nihaomashijie".repeat(4); // 52 字符
        let started = std::time::Instant::now();
        let out = Decoder::new().decode(&input, &lex);
        let elapsed = started.elapsed();
        assert!(!out.candidates.is_empty());
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "52 字符输入解码耗时 {elapsed:?}，词格可能爆炸了"
        );
    }
}
