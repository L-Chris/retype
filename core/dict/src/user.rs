//! 用户词库（L2/L3）：可变、带锁、纯内存，M2 换 SQLite 持久化。
//!
//! 这一层的存在是 test.md 第七节的落点：用户每一次选词、每一次纠错，
//! 都要能反过来改变下一次的候选顺序。

use retype_pinyin::lexicon::LexEntry;
use retype_pinyin::lexicon::Lexicon;
use retype_types::SyllableId;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// 用户词条的初始 logp。
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

#[derive(Debug, Default)]
struct Inner {
    by_key: HashMap<Vec<SyllableId>, Vec<Item>>,
    /// 前缀集合，供 `has_prefix` 剪枝用。用户词库规模小，直接存全前缀。
    prefixes: HashSet<Vec<SyllableId>>,
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
                    text: i.text.clone(),
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
        for layer in &self.layers {
            let start = out.len();
            layer.dict.lookup(syllables, out);
            for e in &mut out[start..] {
                e.logp += layer.boost;
            }
        }
        // 同一个词可能同时出现在系统层和用户层，保留分数更高的那条
        dedup_keeping_best(out);
    }

    fn has_prefix(&self, syllables: &[SyllableId]) -> bool {
        self.layers.iter().any(|l| l.dict.has_prefix(syllables))
    }

    fn lookup_initials(&self, initials: &[u8], out: &mut Vec<LexEntry>) {
        for layer in &self.layers {
            layer.dict.lookup_initials(initials, out);
        }
    }

    fn len(&self) -> usize {
        self.layers.iter().map(|l| l.dict.len()).sum()
    }
}

fn dedup_keeping_best(out: &mut Vec<LexEntry>) {
    if out.len() < 2 {
        return;
    }
    for i in (0..out.len()).rev() {
        let best = (0..i).find(|&j| out[j].text == out[i].text);
        if let Some(j) = best {
            if out[j].logp < out[i].logp {
                out[j].logp = out[i].logp;
            }
            out.remove(i);
        }
    }
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
