//! 用户词库（L2/L3）：可变、带锁的内存索引；存储适配层通过绝对快照恢复。
//!
//! 这一层的存在是 test.md 第七节的落点：用户每一次选词、每一次纠错，
//! 都要能反过来改变下一次的候选顺序。

use crate::adaptive::{Usage, UsageSnapshot, PRIOR_STRENGTH};
use retype_pinyin::lexicon::LexEntry;
use retype_pinyin::lexicon::Lexicon;
use retype_types::{Candidate, CandidateSource, SyllableId};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// 兼容手动 boost 接口的初始 logp。生产学习使用真实词库/分词先验与次数统计。
///
/// **必须和系统词库的 logp 处在同一量级**，否则学习曲线就不存在了：
/// 系统词库的 logp = ln(freq/total)，常见词约 -5、生僻词约 -14。
/// 取 -8.0 相当于「一个中等常见度」的起点，需要用户反复选择才能爬到首位，
/// 一次误点不会把某个词永久钉在第一名。
const USER_BASE_LOGP: f32 = -8.0;

/// 加权上下限。上限保证用户词最终能压过任何系统词（系统最高约 -0.6），
/// 下限保证「不再选择」的词可以被系统词重新盖过。
const BOOST_CEILING: f32 = 2.0;
const BOOST_FLOOR: f32 = -12.0;

#[derive(Debug, Clone)]
struct Item {
    text: Arc<str>,
    logp: f32,
}

#[derive(Debug, Default, Clone)]
struct Inner {
    by_key: HashMap<Vec<SyllableId>, Vec<Item>>,
    /// 前缀集合，供 `has_prefix` 剪枝用。用户词库规模小，直接存全前缀。
    prefixes: HashSet<Vec<SyllableId>>,
    usage: HashMap<Vec<SyllableId>, Vec<Usage>>,
    tick: u64,
}

/// 线程安全的可变用户词库。
///
/// 锁只在单次查询/写入期间持有，**绝不跨 TSF 回调持有**（P1）。
#[derive(Debug, Default)]
pub struct UserDict {
    inner: RwLock<Inner>,
}

impl UserDict {
    pub fn new() -> Self {
        Self::default()
    }

    /// Absolute values for a storage adapter; loading must never apply a boost again.
    pub fn snapshot(&self) -> Vec<(Vec<SyllableId>, String, f32)> {
        self.read()
            .by_key
            .iter()
            .flat_map(|(key, items)| {
                items
                    .iter()
                    .map(|item| (key.clone(), item.text.to_string(), item.logp))
            })
            .collect()
    }

    /// Build the replacement off-lock, then publish it atomically to readers.
    pub fn replace(&self, entries: Vec<(Vec<SyllableId>, String, f32)>) {
        let mut inner = Inner::default();
        for (key, text, logp) in entries {
            if key.is_empty() || text.is_empty() || !logp.is_finite() {
                continue;
            }
            Self::index_prefixes(&mut inner.prefixes, &key);
            inner.by_key.entry(key).or_default().push(Item {
                text: Arc::from(text),
                logp,
            });
        }
        let previous = {
            let mut guard = self.write();
            std::mem::replace(&mut *guard, inner)
        };
        // Deallocating a large old index must not hold the candidate reader's lock.
        drop(previous);
    }

    pub fn copy_from(&self, other: &Self) {
        if std::ptr::eq(self, other) {
            return;
        }
        let replacement = other.read().clone();
        let previous = {
            let mut guard = self.write();
            std::mem::replace(&mut *guard, replacement)
        };
        drop(previous);
    }

    pub fn usage_snapshot(&self) -> UsageSnapshot {
        let inner = self.read();
        UsageSnapshot {
            tick: inner.tick,
            records: inner.usage.values().flatten().cloned().collect(),
        }
    }

    pub fn unresolved_priors(&self) -> Vec<Usage> {
        self.read()
            .usage
            .values()
            .flatten()
            .filter(|record| record.prior_logp.is_none())
            .cloned()
            .collect()
    }

    pub fn restore_usage(&self, snapshot: UsageSnapshot) {
        let mut inner = self.write();
        inner.tick = snapshot.tick;
        inner.usage.clear();
        for mut usage in snapshot.records {
            if usage.count == 0
                || usage.syllables.is_empty()
                || usage.text.is_empty()
                || !usage.recent.is_finite()
                || usage.recent < 0.0
            {
                continue;
            }
            usage.last_tick = usage.last_tick.min(snapshot.tick);
            usage.prior_logp = usage.prior_logp.filter(|score| score.is_finite());
            Self::set_prior(&mut inner, &usage.syllables, &usage.text, usage.prior_logp);
            inner
                .usage
                .entry(usage.syllables.clone())
                .or_default()
                .push(usage);
        }
    }

    fn set_prior(inner: &mut Inner, key: &[SyllableId], text: &str, prior: Option<f32>) {
        // An unresolved reading can be retained without inheriting an old saturated boost.
        let score = prior.unwrap_or(f32::MIN_POSITIVE.ln());
        let entries = inner.by_key.entry(key.to_vec()).or_default();
        if let Some(entry) = entries.iter_mut().find(|entry| &*entry.text == text) {
            entry.logp = score;
        } else {
            entries.push(Item {
                text: Arc::from(text),
                logp: score,
            });
            Self::index_prefixes(&mut inner.prefixes, key);
        }
    }

    pub fn observe(&self, key: &[SyllableId], text: &str, prior: Option<f32>) {
        if key.is_empty() || text.is_empty() {
            return;
        }
        let prior = prior.filter(|score| score.is_finite());
        let mut inner = self.write();
        inner.tick = inner.tick.saturating_add(1);
        let tick = inner.tick;
        let values = inner.usage.entry(key.to_vec()).or_default();
        if let Some(usage) = values.iter_mut().find(|usage| usage.text == text) {
            usage.recent = usage.recent_at(tick) + 1.0;
            usage.count = usage.count.saturating_add(1);
            usage.last_tick = tick;
            if prior.is_some() {
                usage.prior_logp = prior;
            }
        } else {
            values.push(Usage {
                syllables: key.to_vec(),
                text: text.into(),
                count: 1,
                recent: 1.0,
                last_tick: tick,
                prior_logp: prior,
            });
        }
        let prior = values
            .iter()
            .find(|usage| usage.text == text)
            .and_then(|usage| usage.prior_logp);
        Self::set_prior(&mut inner, key, text, prior);
    }

    pub fn resolve_prior(&self, key: &[SyllableId], text: &str, prior: f32) {
        if !prior.is_finite() {
            return;
        }
        let mut inner = self.write();
        if let Some(usage) = inner
            .usage
            .get_mut(key)
            .and_then(|values| values.iter_mut().find(|usage| usage.text == text))
        {
            usage.prior_logp = Some(prior);
            Self::set_prior(&mut inner, key, text, Some(prior));
        }
    }

    /// Merge registered words, retaining the current base score for learned homophones.
    /// Explicit legacy boosts without count records keep their previous behavior.
    fn merge_words(&self, key: &[SyllableId], out: &mut Vec<LexEntry>, start: usize, boost: f32) {
        let inner = self.read();
        let usage = inner.usage.get(key);
        let learned: HashSet<&str> = usage
            .into_iter()
            .flatten()
            .map(|value| value.text.as_str())
            .collect();
        let base: HashSet<Arc<str>> = if usage.is_some() {
            out[start..]
                .iter()
                .map(|entry| Arc::clone(&entry.text))
                .collect()
        } else {
            HashSet::new()
        };
        if let Some(entries) = inner.by_key.get(key) {
            for item in entries {
                if learned.contains(&*item.text) && base.contains(&item.text) {
                    continue;
                }
                out.push(LexEntry {
                    text: Arc::clone(&item.text),
                    logp: item.logp + boost,
                    flags: 0,
                });
            }
        }
        dedup_keeping_best(out, start);
    }

    /// Reweight complete candidates, including phrases composed of several dictionary
    /// words. Each reading/consumption group retains its original aggregate score mass.
    pub fn personalize_candidates(&self, candidates: &mut [Candidate]) -> bool {
        let inner = self.read();
        if inner.usage.is_empty() {
            return false;
        }
        let mut groups: HashMap<Vec<SyllableId>, HashMap<usize, Vec<usize>>> = HashMap::new();
        for (index, candidate) in candidates.iter().enumerate() {
            if candidate.source == CandidateSource::Raw
                || !candidate.score.is_finite()
                || !inner.usage.contains_key(candidate.syllables.as_slice())
            {
                continue;
            }
            let groups_for_reading = match groups.get_mut(candidate.syllables.as_slice()) {
                Some(group) => group,
                None => groups.entry(candidate.syllables.clone()).or_default(),
            };
            groups_for_reading
                .entry(candidate.consumed)
                .or_default()
                .push(index);
        }
        let mut changed = false;
        for (key, groups) in groups {
            let Some(usage) = inner.usage.get(&key) else {
                continue;
            };
            let evidence: HashMap<&str, f64> = usage
                .iter()
                .map(|record| (record.text.as_str(), record.evidence(inner.tick)))
                .collect();
            for indices in groups.into_values() {
                let weights: Vec<f64> = indices
                    .iter()
                    .map(|index| {
                        evidence
                            .get(candidates[*index].text.as_str())
                            .copied()
                            .unwrap_or(0.0)
                    })
                    .collect();
                let total: f64 = weights.iter().sum();
                if total <= 0.0 {
                    continue;
                }
                let maximum = indices
                    .iter()
                    .map(|index| candidates[*index].score as f64)
                    .fold(f64::NEG_INFINITY, f64::max);
                let mass: f64 = indices
                    .iter()
                    .map(|index| (candidates[*index].score as f64 - maximum).exp())
                    .sum();
                let log_mass = maximum + mass.ln();
                for (index, evidence) in indices.into_iter().zip(weights) {
                    let candidate = &mut candidates[index];
                    let base = (candidate.score as f64 - log_mass).exp();
                    let posterior = (PRIOR_STRENGTH * base + evidence) / (PRIOR_STRENGTH + total);
                    let score = (log_mass + posterior.ln()) as f32;
                    changed |= score != candidate.score;
                    candidate.score = score;
                }
            }
        }
        changed
    }

    // 中毒恢复：学习线程 panic 不应该让键盘失灵（P2）
    fn read(&self) -> RwLockReadGuard<'_, Inner> {
        match self.inner.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    fn write(&self) -> RwLockWriteGuard<'_, Inner> {
        match self.inner.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    /// 给已有词加权；词不存在时以 `base_logp` 新建。
    pub fn boost(&self, key: &[SyllableId], text: &str, delta: f32) {
        if key.is_empty() || text.is_empty() {
            return;
        }
        let mut g = self.write();
        let slot = g.by_key.entry(key.to_vec()).or_default();
        if let Some(item) = slot.iter_mut().find(|i| &*i.text == text) {
            item.logp = (item.logp + delta).clamp(BOOST_FLOOR, BOOST_CEILING);
        } else {
            slot.push(Item {
                text: Arc::from(text),
                logp: (USER_BASE_LOGP + delta).clamp(BOOST_FLOOR, BOOST_CEILING),
            });
            Self::index_prefixes(&mut g.prefixes, key);
        }
    }

    /// 自造词 / 纠错后固化。
    pub fn add(&self, text: &str, key: &[SyllableId], logp: f32) {
        self.boost(key, text, logp);
    }

    /// 撤销某个词的加权（设置界面里的「忘记这个词」）。
    pub fn forget(&self, key: &[SyllableId], text: &str) {
        let mut g = self.write();
        if let Some(records) = g.usage.get_mut(key) {
            records.retain(|record| record.text != text);
        }
        if let Some(slot) = g.by_key.get_mut(key) {
            slot.retain(|i| &*i.text != text);
            if slot.is_empty() {
                g.by_key.remove(key);
            }
        }
    }

    pub fn clear(&self) {
        let mut g = self.write();
        g.by_key.clear();
        g.prefixes.clear();
        g.usage.clear();
        g.tick = 0;
    }

    pub fn entry_count(&self) -> usize {
        self.read().by_key.values().map(Vec::len).sum()
    }

    pub fn key_count(&self) -> usize {
        self.read().by_key.len()
    }

    fn index_prefixes(set: &mut HashSet<Vec<SyllableId>>, key: &[SyllableId]) {
        for i in 1..=key.len() {
            set.insert(key[..i].to_vec());
        }
    }
}

impl Lexicon for UserDict {
    fn lookup(&self, syllables: &[SyllableId], out: &mut Vec<LexEntry>) {
        let g = self.read();
        if let Some(items) = g.by_key.get(syllables) {
            for i in items {
                out.push(LexEntry {
                    text: Arc::clone(&i.text),
                    logp: i.logp,
                    flags: 0,
                });
            }
        }
    }

    fn has_prefix(&self, syllables: &[SyllableId]) -> bool {
        self.read().prefixes.contains(syllables)
    }

    fn len(&self) -> usize {
        self.entry_count()
    }
}

/// 系统词库 + 用户词库 + 后续更多层的组合。
///
/// 对应 ARCHITECTURE.md §5 的 L1–L4：越靠上的层越个性化，`boost` 越大。
#[derive(Clone)]
pub struct Layer {
    pub name: &'static str,
    pub dict: Arc<dyn Lexicon>,
    /// 加在 logp 上的偏移。用户层给正值，实现「常用词置顶」。
    pub boost: f32,
}

// `Arc<dyn Lexicon>` 没有 Debug，手写一个对诊断更有用的版本（带词条数）
impl std::fmt::Debug for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer")
            .field("name", &self.name)
            .field("boost", &self.boost)
            .field("entries", &self.dict.len())
            .finish()
    }
}

#[derive(Debug, Default)]
pub struct LayeredDict {
    layers: Vec<Layer>,
    personal: Option<(Arc<UserDict>, f32)>,
}

impl LayeredDict {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, layer: Layer) -> &mut Self {
        self.layers.push(layer);
        self
    }

    /// 常见组合：系统词库在下，用户词库在上。
    pub fn with_system_and_user(
        system: Arc<dyn Lexicon>,
        user: Arc<UserDict>,
        user_boost: f32,
    ) -> Self {
        let mut d = Self::new();
        d.personal = Some((Arc::clone(&user), user_boost));
        d.push(Layer {
            name: "system",
            dict: system,
            boost: 0.0,
        });
        d.push(Layer {
            name: "user",
            dict: user,
            boost: user_boost,
        });
        d
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }
}

impl Lexicon for LayeredDict {
    fn lookup(&self, syllables: &[SyllableId], out: &mut Vec<LexEntry>) {
        let appended_start = out.len();
        for layer in &self.layers {
            if self.personal.is_some() && layer.name == "user" {
                continue;
            }
            let start = out.len();
            layer.dict.lookup(syllables, out);
            for e in &mut out[start..] {
                e.logp += layer.boost;
            }
        }
        // 同一个词可能同时出现在系统层和用户层，保留分数更高的那条
        if let Some((user, boost)) = &self.personal {
            user.merge_words(syllables, out, appended_start, *boost);
        } else {
            dedup_keeping_best(out, appended_start);
        }
    }

    fn has_prefix(&self, syllables: &[SyllableId]) -> bool {
        self.layers.iter().any(|l| l.dict.has_prefix(syllables))
    }

    fn personalize_candidates(&self, candidates: &mut [Candidate]) -> bool {
        self.personal
            .as_ref()
            .is_some_and(|(user, _)| user.personalize_candidates(candidates))
    }

    fn lookup_initials(&self, initials: &[u8], out: &mut Vec<LexEntry>) {
        let appended_start = out.len();
        for layer in &self.layers {
            let start = out.len();
            layer.dict.lookup_initials(initials, out);
            for e in &mut out[start..] {
                e.logp += layer.boost;
            }
        }
        dedup_keeping_best(out, appended_start);
    }

    fn len(&self) -> usize {
        self.layers.iter().map(|l| l.dict.len()).sum()
    }
}

fn dedup_keeping_best(out: &mut Vec<LexEntry>, start: usize) {
    // Lexicon queries append: entries supplied by the caller belong to another
    // query/layer. A nested LayeredDict must never remove or reweight them,
    // otherwise its parent can lose entries and slice past the shortened buffer.
    let mut positions: HashMap<Arc<str>, usize> = HashMap::with_capacity(out.len() - start);
    let mut write = start;
    for i in start..out.len() {
        if let Some(&j) = positions.get(out[i].text.as_ref()) {
            if out[j].logp < out[i].logp {
                out[j].logp = out[i].logp;
            }
        } else {
            positions.insert(Arc::clone(&out[i].text), write);
            if write != i {
                out.swap(write, i);
            }
            write += 1;
        }
    }
    out.truncate(write);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::annotate;

    fn ids(py: &str) -> Vec<SyllableId> {
        annotate::parse_pinyin(py).unwrap()
    }

    #[test]
    fn dedup_preserves_previous_results_first_flags_and_appended_order() {
        let mut out = vec![
            LexEntry::new("甲", -1.0).with_flags(1),
            LexEntry::new("甲", -2.0).with_flags(2),
            LexEntry::new("乙", -8.0).with_flags(3),
            LexEntry::new("丙", -5.0).with_flags(4),
            LexEntry::new("乙", -3.0).with_flags(5),
            LexEntry::new("甲", -9.0).with_flags(6),
            LexEntry::new("丙", -7.0).with_flags(7),
        ];
        dedup_keeping_best(&mut out, 2);
        let rows: Vec<_> = out.iter().map(|e| (&*e.text, e.logp, e.flags)).collect();
        assert_eq!(
            rows,
            [
                ("甲", -1.0, 1),
                ("甲", -2.0, 2),
                ("乙", -3.0, 3),
                ("丙", -5.0, 4),
                ("甲", -9.0, 6)
            ]
        );
    }

    #[test]
    fn boost_makes_word_findable() {
        let u = UserDict::new();
        assert!(!u.has_prefix(&ids("ni hao")));
        u.boost(&ids("ni hao"), "你好", 1.0);
        let mut out = Vec::new();
        u.lookup(&ids("ni hao"), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(&*out[0].text, "你好");
    }

    #[test]
    fn boost_accumulates_and_clamps() {
        let u = UserDict::new();
        for _ in 0..50 {
            u.boost(&ids("ni"), "你", 1.0);
        }
        let mut out = Vec::new();
        u.lookup(&ids("ni"), &mut out);
        assert!(out[0].logp <= BOOST_CEILING, "必须有上限，否则无法再被纠正");
    }

    #[test]
    fn forget_removes_entry() {
        let u = UserDict::new();
        u.boost(&ids("ni"), "你", 2.0);
        u.forget(&ids("ni"), "你");
        let mut out = Vec::new();
        u.lookup(&ids("ni"), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn layered_dedups_and_keeps_best_score() {
        let mut b = crate::memory::DictBuilder::default();
        b.push_word("你好", 5000.0);
        let sys: Arc<dyn Lexicon> = Arc::new(b.build());
        let user = Arc::new(UserDict::new());
        user.boost(&ids("ni hao"), "你好", 3.0);

        let layered = LayeredDict::with_system_and_user(sys, user, 1.0);
        let mut out = Vec::new();
        layered.lookup(&ids("ni hao"), &mut out);
        assert_eq!(out.len(), 1, "系统层与用户层的同一个词必须合并成一条");
        assert_eq!(&*out[0].text, "你好");
    }

    #[test]
    fn layered_prefix_uses_any_layer() {
        let sys: Arc<dyn Lexicon> = Arc::new(crate::memory::DictBuilder::default().build());
        let user = Arc::new(UserDict::new());
        user.boost(&ids("ni hao"), "你好", 1.0);
        let layered = LayeredDict::with_system_and_user(sys, user, 0.0);
        assert!(layered.has_prefix(&ids("ni hao")));
        assert!(!layered.has_prefix(&ids("zhuang")));
    }

    #[derive(Debug)]
    struct FixedLexicon;

    impl Lexicon for FixedLexicon {
        fn lookup(&self, _: &[SyllableId], out: &mut Vec<LexEntry>) {
            out.push(LexEntry::new("脚注", -10.0));
        }

        fn lookup_initials(&self, _: &[u8], out: &mut Vec<LexEntry>) {
            self.lookup(&[], out);
        }

        fn len(&self) -> usize {
            1
        }
    }

    #[test]
    fn nested_layers_append_without_changing_previous_results() {
        // Before the optional nested layer, system and user results still contain
        // duplicates. Even an empty nested layer must leave those entries alone.
        for populated in [false, true] {
            let mut nested = LayeredDict::new();
            if populated {
                for boost in [1.0, 3.0] {
                    nested.push(Layer {
                        name: "pack",
                        dict: Arc::new(FixedLexicon),
                        boost,
                    });
                }
            }
            let mut outer = LayeredDict::new();
            for boost in [0.0, 2.0] {
                outer.push(Layer {
                    name: "base",
                    dict: Arc::new(FixedLexicon),
                    boost,
                });
            }
            outer.push(Layer {
                name: "optional",
                dict: Arc::new(nested),
                boost: 4.0,
            });
            for initials in [false, true] {
                let mut out = vec![
                    LexEntry::new("脚注", -1.0).with_flags(1),
                    LexEntry::new("脚注", -2.0).with_flags(2),
                ];
                if initials {
                    outer.lookup_initials(b"jz", &mut out);
                } else {
                    outer.lookup(&ids("jiao zhu"), &mut out);
                }
                assert_eq!(out.len(), 3);
                assert_eq!((out[0].logp, out[0].flags), (-1.0, 1));
                assert_eq!((out[1].logp, out[1].flags), (-2.0, 2));
                assert_eq!(&*out[2].text, "脚注");
                assert_eq!(out[2].logp, if populated { -3.0 } else { -8.0 });
            }
        }
    }

    #[test]
    fn concurrent_boost_does_not_deadlock_or_lose_data() {
        use std::thread;
        let u = Arc::new(UserDict::new());
        let mut handles = Vec::new();
        for t in 0..4u16 {
            let u = Arc::clone(&u);
            handles.push(thread::spawn(move || {
                for i in 0..50 {
                    u.boost(&ids("ni"), &format!("你{t}{i}"), 0.5);
                }
            }));
        }
        for h in handles {
            h.join().ok();
        }
        assert_eq!(u.entry_count(), 200);
    }
}
