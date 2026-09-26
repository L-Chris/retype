//! 系统词库（L1）：构建期确定的只读词库。
//!
//! 加载必须是**异步**的（ARCHITECTURE.md P1）：TIP 激活时不能在输入线程上读文件。
//! 词库缺失/损坏时降级为「单字模式」——`retype-pinyin` 的音节表 +
//! `annotate::char_readings` 足以给出单字候选，用户仍然能打字（§7 降级矩阵）。

use crate::annotate;
use crate::trie::Trie;
use crate::DictError;
use retype_pinyin::lexicon::{flags, LexEntry, Lexicon};
use retype_types::SyllableId;
use std::io::BufRead;
use std::sync::Arc;

/// 词库里的一条词条。
#[derive(Debug, Clone)]
pub struct Entry {
    pub text: Arc<str>,
    pub logp: f32,
    pub flags: u8,
}

/// 只读系统词库。构建完成后不可变，因此可以安全地被多个线程共享。
#[derive(Debug, Default)]
pub struct MemoryDict {
    entries: Vec<Entry>,
    trie: Trie,
}

impl MemoryDict {
    pub fn builder() -> DictBuilder {
        DictBuilder::default()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// trie 节点数，用于评估内存占用。
    pub fn node_count(&self) -> usize {
        self.trie.node_count()
    }
}

impl Lexicon for MemoryDict {
    fn lookup(&self, syllables: &[SyllableId], out: &mut Vec<LexEntry>) {
        let Some(node) = self.trie.find(syllables) else {
            return;
        };
        for &idx in &node.entries {
            if let Some(e) = self.entries.get(idx as usize) {
                out.push(LexEntry {
                    text: e.text.clone(),
                    logp: e.logp,
                    flags: e.flags,
                });
            }
        }
    }

    fn has_prefix(&self, syllables: &[SyllableId]) -> bool {
        self.trie.contains_prefix(syllables)
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Debug, Clone)]
struct Raw {
    text: Arc<str>,
    ids: Vec<SyllableId>,
    freq: f64,
    flags: u8,
}

/// 词库构建器。收集原始词条 → 归一化词频 → 建 trie。
#[derive(Debug, Default)]
pub struct DictBuilder {
    raw: Vec<Raw>,
}

impl DictBuilder {
    pub fn reserve(&mut self, n: usize) {
        self.raw.reserve(n);
    }

    /// 直接给出音节 id。
    pub fn push(
        &mut self,
        text: &str,
        ids: Vec<SyllableId>,
        freq: f64,
        entry_flags: u8,
    ) -> &mut Self {
        if ids.is_empty() || text.is_empty() {
            return self;
        }
        self.raw.push(Raw {
            text: Arc::from(text),
            ids,
            freq: freq.max(1.0),
            flags: entry_flags,
        });
        self
    }

    /// 给出空格分隔的拼音串，如 `push_pinyin("你好", "ni hao", 5000.0)`。
    pub fn push_pinyin(&mut self, text: &str, py: &str, freq: f64) -> Result<&mut Self, DictError> {
        let ids = annotate::parse_pinyin(py)
            .ok_or_else(|| DictError::UnknownSyllable(format!("{text} -> {py}")))?;
        Ok(self.push(text, ids, freq, 0))
    }

    /// 自动注音。多音字会展开成多个变体（单字展开全部读音）。
    /// 返回实际插入的变体数；非汉字返回 0。
    pub fn push_word(&mut self, text: &str, freq: f64) -> usize {
        let variants = annotate::annotate_variants(text);
        if variants.is_empty() {
            return 0;
        }
        let ambiguous = annotate::is_ambiguous(text);
        let n = variants.len();
        // 多音字变体平分词频：一个词有 3 种读法时，每种读法下的出现概率约为 1/3
        let per = if n > 1 { freq / n as f64 } else { freq };
        for ids in variants {
            let f = if ambiguous { flags::AMBIGUOUS } else { 0 };
            self.push(text, ids, per, f);
        }
        n
    }

    pub fn len(&self) -> usize {
        self.raw.len()
    }

    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    /// 归一化词频为对数概率并建索引。
    pub fn build(mut self) -> MemoryDict {
        // 同 (音节序列, 词) 的重复条目合并词频
        self.raw
            .sort_by(|a, b| a.ids.cmp(&b.ids).then_with(|| a.text.cmp(&b.text)));
        let mut merged: Vec<Raw> = Vec::with_capacity(self.raw.len());
        for r in self.raw.into_iter() {
            match merged.last_mut() {
                Some(last) if last.ids == r.ids && last.text == r.text => {
                    last.freq += r.freq;
                    last.flags |= r.flags;
                }
                _ => merged.push(r),
            }
        }

        let total: f64 = merged.iter().map(|r| r.freq).sum::<f64>().max(1.0);
        let mut entries = Vec::with_capacity(merged.len());
        let mut trie = Trie::with_capacity(merged.len());
        for (idx, r) in merged.into_iter().enumerate() {
            let logp = (r.freq / total).ln() as f32;
            trie.insert(&r.ids, idx as u32);
            entries.push(Entry {
                text: r.text,
                logp,
                flags: r.flags,
            });
        }
        MemoryDict { entries, trie }
    }
}

/// 加载词频表时的过滤规则。
#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// 超过这个字数的词丢弃（输入法里超长词几乎用不到，且会拖慢词格构建）
    pub max_word_len: usize,
    /// 词频下限，滤掉噪声词
    pub min_freq: f64,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            max_word_len: 8,
            min_freq: 1.0,
        }
    }
}

/// 加载统计。用于诊断「词库怎么只剩这么点」。
#[derive(Debug, Clone, Copy, Default)]
pub struct LoadStats {
    pub lines: usize,
    pub accepted: usize,
    pub skipped_malformed: usize,
    pub skipped_non_han: usize,
    pub skipped_too_long: usize,
    pub skipped_low_freq: usize,
    pub variants: usize,
}

/// 读 jieba 词频表格式：`词 词频 [词性]`，空白分隔，`#` 开头为注释。
pub fn load_word_freq<R: BufRead>(
    reader: R,
    opts: &LoadOptions,
) -> Result<(MemoryDict, LoadStats), DictError> {
    let mut stats = LoadStats::default();
    let mut b = DictBuilder::default();

    for line in reader.lines() {
        let line = line?;
        stats.lines += 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let Some(word) = it.next() else {
            stats.skipped_malformed += 1;
            continue;
        };
        let freq = it.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        if freq < opts.min_freq {
            stats.skipped_low_freq += 1;
            continue;
        }
        let n = word.chars().count();
        if n > opts.max_word_len {
            stats.skipped_too_long += 1;
            continue;
        }
        if !annotate::all_han(word) {
            stats.skipped_non_han += 1;
            continue;
        }
        let v = b.push_word(word, freq);
        if v == 0 {
            stats.skipped_non_han += 1;
        } else {
            stats.variants += v;
            stats.accepted += 1;
        }
    }

    if stats.accepted == 0 {
        return Err(DictError::Corrupt(format!(
            "词频表没有产出任何词条（共 {} 行）",
            stats.lines
        )));
    }
    Ok((b.build(), stats))
}

/// 读已注音格式：`词<TAB>拼音<TAB>词频`（`tools/dict-build` 的产物）。
pub fn load_annotated<R: BufRead>(reader: R) -> Result<(MemoryDict, LoadStats), DictError> {
    let mut stats = LoadStats::default();
    let mut b = DictBuilder::default();
    for line in reader.lines() {
        let line = line?;
        stats.lines += 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 2 {
            stats.skipped_malformed += 1;
            continue;
        }
        let freq = cols
            .get(2)
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(1.0);
        match b.push_pinyin(cols[0], cols[1], freq) {
            Ok(_) => stats.accepted += 1,
            Err(_) => stats.skipped_malformed += 1,
        }
    }
    if stats.accepted == 0 {
        return Err(DictError::Corrupt(format!(
            "注音词库没有产出任何词条（共 {} 行）",
            stats.lines
        )));
    }
    Ok((b.build(), stats))
}

/// 单字降级词库：不含任何词，只有全部汉字的单字读音。
///
/// 这是 ARCHITECTURE.md §7 里「系统词库缺失」时的兜底 —— 用户只能打单字，
/// 但仍然打得出来，这比键盘失灵好得多。
pub fn single_char_fallback() -> MemoryDict {
    let mut b = DictBuilder::default();
    for cp in 0x4E00u32..=0x9FFF {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        // 常用度未知，给一个中性词频；真实频率由 M1 的完整词库提供
        b.push_word(&c.to_string(), 10.0);
    }
    b.build()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use retype_pinyin::syllables;
    use std::io::Cursor;

    fn ids(py: &str) -> Vec<SyllableId> {
        annotate::parse_pinyin(py).unwrap()
    }

    fn demo() -> MemoryDict {
        let mut b = DictBuilder::default();
        b.push_word("你", 100000.0);
        b.push_word("好", 80000.0);
        b.push_word("你好", 5000.0);
        b.push_word("吗", 20000.0);
        b.push_word("世界", 9000.0);
        b.push_word("上海", 7000.0);
        b.build()
    }

    #[test]
    fn lookup_returns_entries() {
        let d = demo();
        let mut out = Vec::new();
        d.lookup(&ids("ni hao"), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(&*out[0].text, "你好");
    }

    #[test]
    fn prefix_pruning_works() {
        let d = demo();
        assert!(d.has_prefix(&ids("ni")));
        assert!(d.has_prefix(&ids("ni hao")));
        assert!(!d.has_prefix(&ids("ni hao ma")));
        assert!(!d.has_prefix(&ids("zhuang")));
    }

    #[test]
    fn heteronym_single_char_is_reachable_by_every_reading() {
        let mut b = DictBuilder::default();
        b.push_word("重", 1000.0);
        let d = b.build();
        let mut out = Vec::new();
        d.lookup(&ids("zhong"), &mut out);
        assert!(out.iter().any(|e| &*e.text == "重"));
        out.clear();
        d.lookup(&ids("chong"), &mut out);
        assert!(out.iter().any(|e| &*e.text == "重"));
    }

    #[test]
    fn duplicate_entries_merge_frequency() {
        let mut b = DictBuilder::default();
        b.push_pinyin("你好", "ni hao", 100.0).unwrap();
        b.push_pinyin("你好", "ni hao", 100.0).unwrap();
        let d = b.build();
        let mut out = Vec::new();
        d.lookup(&ids("ni hao"), &mut out);
        assert_eq!(out.len(), 1, "重复词条应合并而不是产生两条");
    }

    #[test]
    fn rarer_word_has_lower_logp() {
        let d = demo();
        let mut a = Vec::new();
        let mut b = Vec::new();
        d.lookup(&ids("ni"), &mut a);
        d.lookup(&ids("ni hao"), &mut b);
        assert!(a[0].logp > b[0].logp, "常用字应比词更「近」");
    }

    #[test]
    fn loads_jieba_format() {
        let txt = "\
你 100000 r
好 80000 a
你好 5000 l
AT&T 3 nz
4S店 3 n
";
        let (d, stats) = load_word_freq(Cursor::new(txt), &LoadOptions::default()).unwrap();
        assert_eq!(stats.accepted, 3);
        assert_eq!(stats.skipped_non_han, 2);
        assert_eq!(d.len(), 3);
    }

    #[test]
    fn empty_word_freq_is_an_error_not_a_silent_zero() {
        let r = load_word_freq(Cursor::new("# nothing\n"), &LoadOptions::default());
        assert!(matches!(r, Err(DictError::Corrupt(_))));
    }

    #[test]
    fn loads_annotated_format() {
        let txt = "你好\tni hao\t5000\n世界\tshi jie\t9000\n";
        let (d, stats) = load_annotated(Cursor::new(txt)).unwrap();
        assert_eq!(stats.accepted, 2);
        let mut out = Vec::new();
        d.lookup(&ids("shi jie"), &mut out);
        assert_eq!(&*out[0].text, "世界");
    }

    #[test]
    fn single_char_fallback_covers_common_chars() {
        let d = single_char_fallback();
        assert!(d.len() > 10000);
        let mut out = Vec::new();
        d.lookup(&ids("zhong"), &mut out);
        assert!(!out.is_empty(), "降级模式下也必须能打出单字");
    }

    #[test]
    fn syllable_ids_are_consistent_with_engine() {
        // 词库与解码器必须共用同一张音节表，否则 id 会错位
        assert_eq!(syllables::id_of("hao"), Some(ids("hao")[0]));
    }
}
