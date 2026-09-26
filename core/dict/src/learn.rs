//! 学习回写（ARCHITECTURE.md §5，对应 test.md 图 7）。
//!
//! 关键性质：**跨输入方式共享**。`LearningEvent` 带 `InputSource` 标签但不分支处理，
//! 所以语音上屏过的专业词会直接影响之后的拼音候选 —— 这正是 test.md 图 5
//! 「输入方式不同，个性化记忆相通」的落点。
//!
//! 所有写入都必须发生在 worker 线程（`SideEffect::Learn`），绝不在输入线程上。

use crate::annotate;
use crate::user::UserDict;
use retype_pinyin::lexicon::{LexEntry, Lexicon};
use retype_types::SyllableId;
use retype_types::{LearningEvent, LearningStore};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// 选词位置 → 加权量。
///
/// 选得越靠后，说明默认排序错得越离谱，信号越强。第 0 位也给一点点正反馈，
/// 否则「一直选首选」的用户永远不会积累任何个人偏好。
///
/// 数值尺度对齐 `USER_BASE_LOGP`（-8.0）：需要**反复**选择才能把一个词顶到首位，
/// 一次误点不足以让某个词永久钉在第一名。
fn chosen_boost(index: usize) -> f32 {
    match index {
        0 => 0.2,
        1 => 0.6,
        2 => 0.9,
        i => 1.2 + (i as f32 * 0.2).min(1.5),
    }
}

/// 纠错对固化所需的重复次数：一次改动可能是手误，反复出现才是偏好。
pub const CORRECTION_CONFIRM_THRESHOLD: u32 = 2;

/// 语音上屏时，单个片段能被收进用户词库的最大字数。
pub const VOICE_WORD_MAX_CHARS: usize = 4;

/// 语音词进入用户词库时的初始加权，低于选词学习（语音识别本身可能出错）。
pub const VOICE_COMMIT_BOOST: f32 = 1.0;

/// 按非汉字字符（标点、空格、字母）切成片段。
fn split_han_clauses(text: &str) -> Vec<&str> {
    text.split(|c: char| !annotate::is_han(c))
        .filter(|s| !s.is_empty())
        .collect()
}

/// 正向最大匹配分词，**复用系统词库的音节 trie**，不引入独立分词器。
///
/// 做法：把片段逐字注音成音节 id，然后在词库里从长到短试匹配，
/// 命中的词必须与原文切片逐字相同（避免同音异形词误判）。
/// 匹配不到的字直接跳过 —— 这里的目标是「捞出真词」，不是完整切分。
fn segment_by_lexicon(clause: &str, lex: &dyn Lexicon) -> Vec<String> {
    let chars: Vec<char> = clause.chars().collect();
    let ids: Vec<Option<SyllableId>> = chars
        .iter()
        .map(|c| annotate::char_readings(*c).first().copied())
        .collect();

    let mut out: Vec<String> = Vec::new();
    let mut buf: Vec<LexEntry> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let mut matched: Option<(usize, String)> = None;
        for len in (2..=VOICE_WORD_MAX_CHARS).rev() {
            if i + len > chars.len() {
                continue;
            }
            let Some(key): Option<Vec<SyllableId>> = ids[i..i + len].iter().copied().collect()
            else {
                continue;
            };
            buf.clear();
            lex.lookup(&key, &mut buf);
            let want: String = chars[i..i + len].iter().collect();
            if buf.iter().any(|e| &*e.text == want) {
                matched = Some((len, want));
                break;
            }
        }
        match matched {
            Some((len, w)) => {
                out.push(w);
                i += len;
            }
            None => i += 1,
        }
    }
    out
}

/// 内存学习器。M2 换成 SQLite 持久化实现（同一个 `LearningStore` trait）。
pub struct Learner {
    user: Arc<UserDict>,
    /// 系统词库，用于从语音长句里捞出真词（见 `segment_by_lexicon`）
    system: Option<Arc<dyn Lexicon>>,
    /// `(原始, 修改后) -> 出现次数`
    corrections: RwLock<HashMap<(String, String), u32>>,
    events: RwLock<Vec<LearningEvent>>,
    keep_events: usize,
}

impl std::fmt::Debug for Learner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Learner")
            .field("user_entries", &self.user.entry_count())
            .field("has_system_dict", &self.system.is_some())
            .field("corrections", &self.correction_kind_count())
            .field("events", &self.event_count())
            .finish()
    }
}

impl Learner {
    pub fn new(user: Arc<UserDict>) -> Self {
        Self {
            user,
            system: None,
            corrections: RwLock::new(HashMap::new()),
            events: RwLock::new(Vec::new()),
            keep_events: 512,
        }
    }

    /// 带上系统词库的学习器：能从语音长句里分词并提取真词。
    pub fn with_system(user: Arc<UserDict>, system: Arc<dyn Lexicon>) -> Self {
        Self {
            system: Some(system),
            ..Self::new(user)
        }
    }

    pub fn user(&self) -> &Arc<UserDict> {
        &self.user
    }

    pub fn correction_count(&self, from: &str, to: &str) -> u32 {
        let key = (from.to_owned(), to.to_owned());
        Self::read_map(&self.corrections)
            .get(&key)
            .copied()
            .unwrap_or(0)
    }

    /// 出现次数最多的纠错对，供云端 final pass 做后处理（M4）。
    pub fn top_corrections(&self, n: usize) -> Vec<(String, String, u32)> {
        let mut v: Vec<(String, String, u32)> = Self::read_map(&self.corrections)
            .iter()
            .map(|((a, b), c)| (a.clone(), b.clone(), *c))
            .collect();
        v.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    pub fn correction_kind_count(&self) -> usize {
        Self::read_map(&self.corrections).len()
    }

    pub fn event_count(&self) -> usize {
        match self.events.read() {
            Ok(g) => g.len(),
            Err(p) => p.into_inner().len(),
        }
    }

    /// 中毒恢复：学习线程出问题不能影响打字（P2）
    fn read_map<'a>(
        lock: &'a RwLock<HashMap<(String, String), u32>>,
    ) -> std::sync::RwLockReadGuard<'a, HashMap<(String, String), u32>> {
        match lock.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    fn push_event(&self, e: LearningEvent) {
        let mut g = match self.events.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        g.push(e);
        let overflow = g.len().saturating_sub(self.keep_events);
        if overflow > 0 {
            g.drain(0..overflow);
        }
    }

    /// 语音上屏的文字反哺拼音候选（test.md 图 5 的闭环）。
    ///
    /// 整句不能直接当词条，否则用户词库会被长句塞满、还会拖慢词格构建。
    /// 所以只收「2~4 个汉字、不含标点」的片段；真正的分词切分留给 M2。
    fn record_voice_commit(&self, text: &str) {
        for seg in split_han_clauses(text) {
            let n = seg.chars().count();
            if (2..=VOICE_WORD_MAX_CHARS).contains(&n) && annotate::all_han(seg) {
                self.add_word(seg, VOICE_COMMIT_BOOST);
                continue;
            }
            // 长句：用系统词库做正向最大匹配，只捞出真实存在的词
            if let Some(sys) = &self.system {
                for w in segment_by_lexicon(seg, sys.as_ref()) {
                    self.add_word(&w, VOICE_COMMIT_BOOST);
                }
            }
        }
    }

    fn add_word(&self, word: &str, boost: f32) {
        if annotate::all_han(word) {
            if let Some(ids) = annotate::annotate(word) {
                self.user.add(word, &ids, boost);
            }
        }
    }

    fn record_correction(&self, from: &str, to: &str) {
        let key = (from.to_owned(), to.to_owned());
        let count = {
            let mut g = match self.corrections.write() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let c = g.entry(key.clone()).or_insert(0);
            *c += 1;
            *c
        };
        // 反复出现才固化成用户词，避免把手误学进去
        if count >= CORRECTION_CONFIRM_THRESHOLD && annotate::all_han(to) {
            if let Some(ids) = annotate::annotate(to) {
                self.user.add(to, &ids, 1.5);
            }
        }
    }
}

impl LearningStore for Learner {
    fn record(&self, event: LearningEvent) {
        match &event {
            LearningEvent::CandidateChosen {
                text,
                syllables,
                index,
                ..
            } => {
                if !syllables.is_empty() {
                    self.user.boost(syllables, text, chosen_boost(*index));
                }
            }
            LearningEvent::Corrected { from, to, .. } => self.record_correction(from, to),
            LearningEvent::Coinage { text, syllables } => {
                if !syllables.is_empty() {
                    self.user.add(text, syllables, 1.5);
                }
            }
            LearningEvent::VoiceCommit { text } => self.record_voice_commit(text),
        }
        self.push_event(event);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::annotate;
    use retype_pinyin::Lexicon;
    use retype_types::InputSource;
    use retype_types::SyllableId;

    fn ids(py: &str) -> Vec<SyllableId> {
        annotate::parse_pinyin(py).unwrap()
    }

    fn setup() -> (Arc<UserDict>, Learner) {
        let u = Arc::new(UserDict::new());
        let l = Learner::new(Arc::clone(&u));
        (u, l)
    }

    #[test]
    fn choosing_a_lower_candidate_boosts_it() {
        let (u, l) = setup();
        l.record(LearningEvent::CandidateChosen {
            source: InputSource::Keyboard,
            text: "你好".into(),
            syllables: ids("ni hao"),
            index: 3,
        });
        let mut out = Vec::new();
        u.lookup(&ids("ni hao"), &mut out);
        assert_eq!(out.len(), 1);
        // 起点是 USER_BASE_LOGP(-8.0)，一次选择只应把它抬起来一点，
        // 不足以立刻压过系统词库里的常见词
        assert!(
            out[0].logp > -7.0,
            "第 4 位被选中应给出可观测的加权，实际 {}",
            out[0].logp
        );
        assert!(out[0].logp < -3.0, "但一次选择绝不能让它直接登顶");
    }

    #[test]
    fn repeated_choices_eventually_promote_a_word() {
        let (u, l) = setup();
        for _ in 0..12 {
            l.record(LearningEvent::CandidateChosen {
                source: InputSource::Keyboard,
                text: "你好".into(),
                syllables: ids("ni hao"),
                index: 1,
            });
        }
        let mut out = Vec::new();
        u.lookup(&ids("ni hao"), &mut out);
        // -8.0 起步，12 × 0.6 = 7.2 → 约 -0.8，已经达到「高频系统词」的量级
        assert!(
            out[0].logp >= -1.0,
            "反复选择后应能压过系统词，实际 {}",
            out[0].logp
        );
    }

    #[test]
    fn voice_commits_boost_pinyin_candidates() {
        // test.md 图 5：语音学过的词要能影响键盘候选
        let (u, l) = setup();
        l.record(LearningEvent::CandidateChosen {
            source: InputSource::Voice,
            text: "奥司他韦".into(),
            syllables: ids("ao si ta wei"),
            index: 2,
        });
        let mut out = Vec::new();
        u.lookup(&ids("ao si ta wei"), &mut out);
        assert!(out.iter().any(|e| &*e.text == "奥司他韦"));
    }

    #[test]
    fn single_correction_is_not_learned_as_a_word() {
        let (u, l) = setup();
        l.record(LearningEvent::Corrected {
            from: "人工只能".into(),
            to: "人工智能".into(),
            context_hash: 1,
        });
        assert_eq!(l.correction_count("人工只能", "人工智能"), 1);
        let mut out = Vec::new();
        u.lookup(&ids("ren gong zhi neng"), &mut out);
        assert!(out.is_empty(), "一次改动可能是手误，不该立刻进用户词库");
    }

    #[test]
    fn repeated_correction_becomes_a_user_word() {
        let (u, l) = setup();
        for _ in 0..CORRECTION_CONFIRM_THRESHOLD {
            l.record(LearningEvent::Corrected {
                from: "人工只能".into(),
                to: "人工智能".into(),
                context_hash: 1,
            });
        }
        let mut out = Vec::new();
        u.lookup(&ids("ren gong zhi neng"), &mut out);
        assert!(out.iter().any(|e| &*e.text == "人工智能"));
        assert_eq!(
            l.top_corrections(1),
            vec![("人工只能".to_string(), "人工智能".to_string(), 2)]
        );
    }

    #[test]
    fn coinage_is_added() {
        let (u, l) = setup();
        l.record(LearningEvent::Coinage {
            text: "重构计划".into(),
            syllables: ids("chong gou ji hua"),
        });
        let mut out = Vec::new();
        u.lookup(&ids("chong gou ji hua"), &mut out);
        assert!(out.iter().any(|e| &*e.text == "重构计划"));
    }

    #[test]
    fn voice_commit_feeds_back_into_pinyin_candidates() {
        // test.md 图 5：语音说过的专业词，之后打拼音要能选到
        let (sys, _) = crate::from_pairs([
            ("人工智能", "ren gong zhi neng", 5000.0),
            ("研究", "yan jiu", 4000.0),
            ("大模型", "da mo xing", 3000.0),
        ]);
        let u = Arc::new(UserDict::new());
        let l = Learner::with_system(Arc::clone(&u), Arc::new(sys));
        l.record(LearningEvent::VoiceCommit {
            text: "我们在研究人工智能大模型。".into(),
        });
        let mut out = Vec::new();
        u.lookup(&ids("ren gong zhi neng"), &mut out);
        assert!(
            out.iter().any(|e| &*e.text == "人工智能"),
            "应从句子里分出「人工智能」"
        );
        out.clear();
        u.lookup(&ids("yan jiu"), &mut out);
        assert!(out.iter().any(|e| &*e.text == "研究"));
    }

    #[test]
    fn short_voice_utterance_is_learned_without_system_dict() {
        let (u, l) = setup();
        l.record(LearningEvent::VoiceCommit {
            text: "奥司他韦".into(),
        });
        let mut out = Vec::new();
        u.lookup(&ids("ao si ta wei"), &mut out);
        assert!(out.iter().any(|e| &*e.text == "奥司他韦"));
    }

    #[test]
    fn long_clause_is_not_learned_as_one_word() {
        let (u, l) = setup();
        l.record(LearningEvent::VoiceCommit {
            text: "这一段话实在太长了不适合当成一个词条".into(),
        });
        assert_eq!(u.entry_count(), 0, "长句不该被整句塞进用户词库");
    }

    #[test]
    fn event_log_is_bounded() {
        let (_, l) = setup();
        for _ in 0..2000 {
            l.record(LearningEvent::Coinage {
                text: "词".into(),
                syllables: ids("ci"),
            });
        }
        assert_eq!(l.event_count(), 512, "事件日志必须有界，否则常驻内存会涨");
    }
}
