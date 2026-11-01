//! 词格构建 + k-best Viterbi 解码（首刷的算法核心）。
//!
//! 设计要点（对应 ARCHITECTURE.md §2）：
//! - **音节边界歧义由词库解决，不由切分器猜**：切分器把所有合法切分铺成一张词格，
//!   Viterbi 结合词频挑最优路径。`xian` / `jian` / `shanghai` 这类歧义天然被覆盖。
//! - 所有方法都是纯内存计算，必须在输入线程上同步完成（P1），按键 P99 预算 ≤ 6ms。
//! - 词格扩展靠 [`Lexicon::has_prefix`] 剪枝，否则组合数会随输入长度指数爆炸。

use crate::lexicon::{LexEntry, Lexicon};
use crate::syllables;
use retype_types::{Candidate, CandidateSource, SyllableId};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

/// 解码参数。
#[derive(Debug, Clone)]
pub struct DecodeOptions {
    /// 保留几条完整路径（k-best）
    pub k: usize,
    /// 候选窗每页条数
    pub page_size: usize,
    /// 自动组词时每个词边的加分。
    ///
    /// unigram 模型下 `logp(w) = ln(freq_w / T)`，一条 k 词路径的总分是
    /// `Σ ln f_i − k·ln T`：每多切一个词就多扣一次 ln T。
    /// 这是模型的分词偏差，不能表达相邻词之间的真实搭配概率。
    ///
    /// 仅在没有完整词条、需要自动组词时使用；完整词条由独立路线处理。
    /// `word_bonus` 就是这个偏差的部分补偿。理论上界是 `ln T`（完全补偿，
    /// 等价于假设任意词都能自由相接，结果会退化成全单字）；为 0 时倾向过度合并。
    /// 后续引入搭配模型时需要重新评估；当前不靠调整该值实现整词优先。
    pub word_bonus: f32,
    /// 无法匹配任何音节时，单个原样字母的惩罚。必须比最生僻的字更差，
    /// 否则解码器会宁可直接吐字母。
    pub raw_penalty: f32,
    /// 词格扩展的最大音节数（中文词绝大多数 ≤ 5 音节）
    pub max_word_syllables: usize,
    /// 是否额外产出「前缀候选」（选一个短词后继续组字，是拼音输入法的核心交互）
    pub include_prefixes: bool,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            k: 24,
            page_size: 9,
            word_bonus: 3.5,
            raw_penalty: 20.0,
            max_word_syllables: 8,
            include_prefixes: true,
        }
    }
}

/// 归一化用户输入：转小写、`ü`→`v`、丢弃非 `a-z` 与 `'` 的字符。
pub fn normalize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '\'' => out.push('\''),
            'ü' | 'Ü' => out.push('v'),
            _ => {
                let c = c.to_ascii_lowercase();
                if c.is_ascii_lowercase() {
                    out.push(c);
                }
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
enum EdgeText {
    /// 词库命中
    Word(Arc<str>),
    /// 无法匹配的字母，原样保留（保证任何输入都有路径可走）
    Raw,
    /// 显式分隔符 `'`，消费但不产出文字
    Skip,
}

#[derive(Debug, Clone)]
struct Edge {
    to: usize,
    text: EdgeText,
    syllables: Vec<SyllableId>,
    score: f32,
}

/// 词格：`edges[i]` 是从字符位置 `i` 出发的所有边。
#[derive(Debug)]
pub struct Lattice {
    n: usize,
    edges: Vec<Vec<Edge>>,
    pub(crate) complete_tail: bool,
}

impl Lattice {
    pub fn len(&self) -> usize {
        self.n
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    pub fn edge_count(&self) -> usize {
        self.edges.iter().map(Vec::len).sum()
    }

    fn whole_word_edges<'a>(
        &'a self,
        input: &str,
    ) -> impl Iterator<Item = (usize, usize, &'a Edge)> {
        let start = input.len() - input.trim_start_matches('\'').len();
        let end = input.trim_end_matches('\'').len();
        self.edges[start]
            .iter()
            .enumerate()
            .filter_map(move |(ei, edge)| {
                (start < end && edge.to == end && matches!(edge.text, EdgeText::Word(_)))
                    .then_some((start, ei, edge))
            })
    }

    pub(crate) fn has_whole_word(&self, input: &str) -> bool {
        self.whole_word_edges(input).next().is_some()
    }
}

/// 从 `pos` 起所有合法的音节切分（长度 1..=6，遇 `'` 截断）。
fn syllable_options(input: &[u8], pos: usize, out: &mut Vec<(usize, SyllableId)>) {
    out.clear();
    let remain = input.len().saturating_sub(pos);
    let max = syllables::MAX_SYLLABLE_LEN.min(remain);
    for len in 1..=max {
        if input[pos + len - 1] == b'\'' {
            break;
        }
        let Ok(s) = std::str::from_utf8(&input[pos..pos + len]) else {
            continue;
        };
        if let Some(id) = syllables::id_of(s) {
            out.push((len, id));
        }
    }
}

/// 构建词格。
pub fn build_lattice(input: &str, lex: &dyn Lexicon, opts: &DecodeOptions) -> Lattice {
    build_lattice_with(input, lex, opts, syllable_options)
}

pub(crate) fn build_lattice_with(
    input: &str,
    lex: &dyn Lexicon,
    opts: &DecodeOptions,
    options: impl Fn(&[u8], usize, &mut Vec<(usize, SyllableId)>),
) -> Lattice {
    build_lattice_with_queries(input, &mut LatticeQueries::new(lex, false), opts, options)
}

/// Scoped to one decode: dictionary/learning changes are observed on the next
/// key. Share raw lookup results, never personalized candidates or rankings.
pub(crate) struct LatticeQueries<'a> {
    lex: &'a dyn Lexicon,
    cached: HashMap<Vec<SyllableId>, Option<Vec<LexEntry>>>,
    entries: Vec<LexEntry>,
    entry_count: usize,
    memoize: bool,
}

#[cfg(test)]
mod query_tests {
    use super::*;

    struct Lex;
    impl Lexicon for Lex {
        fn has_prefix(&self, ids: &[SyllableId]) -> bool {
            ids != [0]
        }
        fn lookup(&self, ids: &[SyllableId], out: &mut Vec<LexEntry>) {
            if ids == [1] {
                // A reachable prefix without a terminal is not a failed prefix.
                return;
            }
            let count = if ids == [2] { 4097 } else { 1 };
            out.extend((0..count).map(|_| LexEntry::new("字", -1.0)));
        }
        fn len(&self) -> usize {
            1
        }
    }

    #[test]
    fn query_limits_fall_back_without_truncating_or_confusing_empty_prefixes() {
        let mut queries = LatticeQueries::new(&Lex, true);
        for _ in 0..2 {
            assert!(queries.lookup(&[0]).is_none());
            assert_eq!(queries.lookup(&[1]).map(<[_]>::len), Some(0));
            assert_eq!(queries.lookup(&[2]).map(<[_]>::len), Some(4097));
        }
        for id in 3..400 {
            assert_eq!(queries.lookup(&[id]).map(<[_]>::len), Some(1));
        }
        assert_eq!(queries.cached.len(), 256);
        assert!(queries.entry_count <= 4096);
        assert!(!queries.cached.contains_key([2].as_slice()));
        assert_eq!(queries.lookup(&[399]).map(<[_]>::len), Some(1));
    }
}

impl<'a> LatticeQueries<'a> {
    pub(crate) fn new(lex: &'a dyn Lexicon, memoize: bool) -> Self {
        Self {
            lex,
            cached: HashMap::new(),
            entries: Vec::new(),
            entry_count: 0,
            memoize,
        }
    }

    fn lookup(&mut self, ids: &[SyllableId]) -> Option<&[LexEntry]> {
        if self.memoize && self.cached.contains_key(ids) {
            return self.cached.get(ids).and_then(|hits| hits.as_deref());
        }
        self.entries.clear();
        let exists = self.lex.has_prefix(ids);
        if exists {
            self.lex.lookup(ids, &mut self.entries);
        }
        // Bound retained entries as well as keys. Large homophone lists keep
        // using the scratch buffer rather than duplicating them in the cache.
        if self.memoize
            && ids.len() <= 32
            && self.cached.len() < 256
            && self.entry_count + self.entries.len() <= 4096
        {
            self.entry_count += self.entries.len();
            let hits = exists.then(|| std::mem::take(&mut self.entries));
            return self.cached.entry(ids.to_vec()).or_insert(hits).as_deref();
        }
        exists.then_some(self.entries.as_slice())
    }
}

pub(crate) fn build_lattice_with_queries(
    input: &str,
    queries: &mut LatticeQueries<'_>,
    opts: &DecodeOptions,
    options: impl Fn(&[u8], usize, &mut Vec<(usize, SyllableId)>),
) -> Lattice {
    let bytes = input.as_bytes();
    let n = bytes.len();
    let mut edges: Vec<Vec<Edge>> = vec![Vec::new(); n + 1];
    let mut syl_opts: Vec<(usize, SyllableId)> = Vec::new();
    let mut stack: Vec<(usize, Vec<SyllableId>)> = Vec::new();
    let mut reachable = vec![false; n + 1];
    reachable[0] = true;

    for i in 0..n {
        // Only expand boundaries reachable by a dictionary word, separator or
        // raw fallback from the start. Building overlapping graphs inside a
        // word cannot contribute to any Viterbi path or selectable prefix.
        if !reachable[i] {
            continue;
        }
        if bytes[i] == b'\'' {
            // 显式分隔符：消费掉但不产出文字，也不参与切分
            edges[i].push(Edge {
                to: i + 1,
                text: EdgeText::Skip,
                syllables: Vec::new(),
                score: 0.0,
            });
            reachable[i + 1] = true;
            continue;
        }

        stack.clear();
        stack.push((i, Vec::new()));
        while let Some((mut pos, syls)) = stack.pop() {
            // An explicit separator fixes a syllable boundary, but does not
            // prevent one dictionary phrase from spanning several syllables.
            while bytes.get(pos) == Some(&b'\'') {
                pos += 1;
            }
            options(bytes, pos, &mut syl_opts);
            for (len, id) in syl_opts.iter().copied() {
                let npos = pos + len;
                let mut next = syls.clone();
                next.push(id);
                let Some(entries) = queries.lookup(&next) else {
                    continue;
                };
                for e in entries.iter() {
                    edges[i].push(Edge {
                        to: npos,
                        text: EdgeText::Word(Arc::clone(&e.text)),
                        syllables: next.clone(),
                        score: e.logp + opts.word_bonus,
                    });
                }
                if next.len() < opts.max_word_syllables && npos < n {
                    stack.push((npos, next));
                }
            }
        }

        // 兜底边只在「这个位置一个词都接不上」时才铺。
        //
        // 早先版本在每个位置都无条件铺一条 Raw 边，结果 k-best 的束槽位被
        // 「你好ma世界」这种词+字母混排的垃圾路径占掉（真实词库上表现为
        // 「妳好吗世界」压过「你好吗世界」）。现在只有真正无法切分时才退化，
        // 同时 DP 依然不会走进死胡同。
        if edges[i].is_empty() {
            edges[i].push(Edge {
                to: i + 1,
                text: EdgeText::Raw,
                syllables: Vec::new(),
                score: -opts.raw_penalty,
            });
        }
        for edge in &edges[i] {
            reachable[edge.to] = true;
        }
    }

    Lattice {
        n,
        edges,
        complete_tail: false,
    }
}

/// k-best 搜索节点。
///
/// **用链表而不是「(前驱位置, 槽位下标)」回溯**：每层的 top-k 是边扩展边插入的，
/// `insert_node` 会在中间插入并 `truncate` 末尾，所以早先记下的槽位下标会失效，
/// 回溯时会拼出一条实际不存在的路径（表现为候选里冒出莫名其妙的字）。
/// `Rc` 链的额外开销很小：每个节点只在被引用时存活，共享前缀天然去重。
#[derive(Clone)]
struct Node {
    score: f32,
    from: u32,
    edge: u32,
    prev: Option<Rc<Node>>,
}

fn insert_node(beam: &mut Vec<Rc<Node>>, node: Rc<Node>, k: usize) {
    if k == 0 {
        return;
    }
    // 降序插入：找到第一个分数严格更小的位置。k 很小（默认 8），线性扫描比
    // 浮点 binary_search 更快也更直观。
    let mut idx = beam.len();
    for (i, b) in beam.iter().enumerate() {
        if b.score < node.score {
            idx = i;
            break;
        }
    }
    if idx >= k {
        return;
    }
    beam.insert(idx, node);
    beam.truncate(k);
}

/// k-best 束搜索。返回按分数降序的路径，每条路径是 `[(起点位置, 边下标)]`。
fn kbest(lattice: &Lattice, k: usize) -> Vec<(f32, Vec<(usize, usize)>)> {
    let n = lattice.n;
    let mut dp: Vec<Vec<Rc<Node>>> = vec![Vec::new(); n + 1];
    // Keep dictionary-word completions in their own bounded beam. Otherwise
    // frequent single characters at an unfinished final syllable can evict
    // every path ending in a phrase before candidate ranking even sees it.
    let mut completions: Vec<Rc<Node>> = Vec::new();
    // 起点哨兵：prev == None 标记路径开头
    dp[0].push(Rc::new(Node {
        score: 0.0,
        from: 0,
        edge: 0,
        prev: None,
    }));

    for i in 0..n {
        if dp[i].is_empty() {
            continue;
        }
        // 只克隆 Rc（引用计数 +1），不复制节点
        let cur = dp[i].clone();
        for beam in cur.iter() {
            for (ei, e) in lattice.edges[i].iter().enumerate() {
                let score = beam.score + e.score;
                let destination = &dp[e.to];
                let keep_regular = destination.len() < k || score > destination[k - 1].score;
                let keep_completion = lattice.complete_tail
                    && e.to == n
                    && e.syllables.len() > 1
                    && (completions.len() < k || score > completions[k - 1].score);
                if !keep_regular && !keep_completion {
                    continue;
                }
                let node = Rc::new(Node {
                    score,
                    from: i as u32,
                    edge: ei as u32,
                    prev: Some(Rc::clone(beam)),
                });
                if keep_completion {
                    insert_node(&mut completions, Rc::clone(&node), k);
                }
                if keep_regular {
                    insert_node(&mut dp[e.to], node, k);
                }
            }
        }
    }

    let mut paths = Vec::new();
    for node in dp[n].iter().chain(completions.iter()) {
        let mut rev: Vec<(usize, usize)> = Vec::new();
        let mut cur = Some(Rc::clone(node));
        // 路径长度上界 = 字符数，超过说明状态异常，直接止损
        let mut guard = n + 2;
        while let Some(nd) = cur {
            if nd.prev.is_none() || guard == 0 {
                break;
            }
            guard -= 1;
            rev.push((nd.from as usize, nd.edge as usize));
            cur = nd.prev.clone();
        }
        rev.reverse();
        paths.push((node.score, rev));
    }
    paths.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    paths
}

/// Reliable whole-word matches and automatic composition are separate routes.
/// Preserve every dictionary homophone (also beyond k), and only run the
/// bounded sentence search when no word covers the complete input.
fn decode_paths(
    input: &str,
    lattice: &Lattice,
    opts: &DecodeOptions,
) -> Vec<(f32, Vec<(usize, usize)>)> {
    let mut paths: Vec<_> = lattice
        .whole_word_edges(input)
        .map(|(start, ei, edge)| {
            let mut path: Vec<_> = (0..start).map(|pos| (pos, 0)).collect();
            path.push((start, ei));
            path.extend((edge.to..lattice.n).map(|pos| (pos, 0)));
            (edge.score, path)
        })
        .collect();
    if paths.is_empty() {
        return kbest(lattice, opts.k.max(1));
    }
    paths.sort_by(|a, b| b.0.total_cmp(&a.0));
    paths
}

/// 解码结果。
#[derive(Debug, Clone, Default)]
pub struct DecodeOutput {
    /// A single dictionary word covers the input, rather than automatic composition.
    pub has_whole_word: bool,
    /// 已排序去重的候选（完整句候选在前，前缀词候选在后）
    pub candidates: Vec<Candidate>,
    /// 首选路径的音节切分，用于显示 `ni'hao'ma`
    pub syllables: Vec<String>,
    /// 首选路径消费掉的音节数
    pub matched_syllables: usize,
    /// 首选路径是否含未能匹配的字母（true 表示还在打无效串）
    pub has_raw: bool,
    /// Full-coverage candidates whose unfinished final syllable completes a
    /// dictionary phrase. Internal evidence, independent of display text length.
    pub(crate) word_completions: HashSet<String>,
}

fn comment_of(syllables: &[SyllableId]) -> String {
    let mut comment = String::with_capacity(syllables.len() * (syllables::MAX_SYLLABLE_LEN + 1));
    for name in syllables.iter().filter_map(|id| syllables::name_of(*id)) {
        if !comment.is_empty() {
            comment.push('\'');
        }
        comment.push_str(name);
    }
    comment
}

fn word_candidate(edge: &Edge, consumed: usize) -> Option<Candidate> {
    let EdgeText::Word(text) = &edge.text else {
        return None;
    };
    Some(Candidate {
        language: Default::default(),
        text: text.to_string(),
        comment: comment_of(&edge.syllables),
        source: CandidateSource::Local,
        syllable_len: edge.syllables.len(),
        consumed,
        syllables: edge.syllables.clone(),
        score: edge.score,
    })
}

fn path_to_candidate(
    input: &[u8],
    lattice: &Lattice,
    path: &[(usize, usize)],
    score: f32,
) -> Option<(Candidate, bool)> {
    let mut text = String::new();
    let mut ids: Vec<SyllableId> = Vec::new();
    let mut has_raw = false;
    let mut consumed = 0usize;
    for (from, ei) in path {
        let edge = lattice.edges.get(*from)?.get(*ei)?;
        consumed += edge.to.saturating_sub(*from);
        match &edge.text {
            EdgeText::Word(t) => text.push_str(t),
            EdgeText::Raw => {
                has_raw = true;
                if let Some(b) = input.get(*from) {
                    text.push(*b as char);
                }
            }
            EdgeText::Skip => {}
        }
        ids.extend_from_slice(&edge.syllables);
    }
    if text.is_empty() {
        return None;
    }
    let len = ids.len();
    let comment = comment_of(&ids);
    Some((
        Candidate {
            language: Default::default(),
            text,
            comment,
            source: if has_raw {
                CandidateSource::Raw
            } else {
                CandidateSource::Local
            },
            syllable_len: len,
            consumed,
            syllables: ids,
            score,
        },
        has_raw,
    ))
}

/// 前缀候选：沿着最优路径逐词切出「先上一部分」的候选。
///
/// 这是拼音输入法的核心交互 —— 用户打 `nihaomashijie`，
/// 可以只选「你好」，剩下的 `mashijie` 继续组字。
fn prefix_candidates(lattice: &Lattice, path: &[(usize, usize)]) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut ids: Vec<SyllableId> = Vec::new();
    let mut consumed = 0usize;
    let mut score = 0.0;
    let mut ended_on_word = false;
    for (from, ei) in path {
        let Some(edge) = lattice.edges.get(*from).and_then(|v| v.get(*ei)) else {
            break;
        };
        consumed += edge.to.saturating_sub(*from);
        match &edge.text {
            EdgeText::Word(t) => {
                text.push_str(t);
                ids.extend_from_slice(&edge.syllables);
                score += edge.score;
                ended_on_word = true;
            }
            // 一旦遇到原样字母，后面的前缀就没有意义了
            EdgeText::Raw | EdgeText::Skip => {
                ended_on_word = false;
                break;
            }
        }
        out.push(Candidate {
            language: Default::default(),
            text: text.clone(),
            comment: comment_of(&ids),
            source: if ids.len() == 1 {
                CandidateSource::SingleChar
            } else {
                CandidateSource::Local
            },
            syllable_len: ids.len(),
            consumed,
            syllables: ids.clone(),
            score,
        });
    }
    // 完整路径已由 k-best 产出；若遇到原样字母，则保留它之前的最后一个中文词。
    if ended_on_word {
        out.pop();
    }
    out
}

/// 把归一化后的输入解码成候选列表。
pub fn decode(input: &str, lex: &dyn Lexicon, opts: &DecodeOptions) -> DecodeOutput {
    if input.is_empty() {
        return DecodeOutput::default();
    }
    let lattice = build_lattice(input, lex, opts);
    decode_lattice(input, lattice, opts, lex)
}

pub(crate) fn decode_lattice(
    input: &str,
    lattice: Lattice,
    opts: &DecodeOptions,
    lex: &dyn Lexicon,
) -> DecodeOutput {
    let mut whole_words: Vec<_> = lattice
        .whole_word_edges(input)
        .map(|(_, _, edge)| edge)
        .collect();
    whole_words.sort_by(|a, b| b.score.total_cmp(&a.score));
    let has_whole_word = !whole_words.is_empty();
    // Direct matches do not need path allocation or backtracking. This matters
    // for single-key previews with thousands of dictionary homophones.
    let paths = if has_whole_word {
        Vec::new()
    } else {
        kbest(&lattice, opts.k.max(1))
    };
    let bytes = input.as_bytes();

    let capacity = whole_words.len().max(paths.len());
    let mut found: Vec<(Candidate, bool, Option<usize>)> = Vec::with_capacity(capacity);
    let mut seen: HashSet<String> = HashSet::with_capacity(capacity);
    let mut word_completions = HashSet::new();

    for edge in whole_words {
        let Some(candidate) = word_candidate(edge, input.len()) else {
            continue;
        };
        if seen.insert(candidate.text.clone()) {
            found.push((candidate, false, None));
        }
    }

    for (pi, (score, path)) in paths.iter().enumerate() {
        let Some((cand, raw)) = path_to_candidate(bytes, &lattice, path, *score) else {
            continue;
        };
        if lattice.complete_tail
            && !raw
            && path
                .last()
                .is_some_and(|&(from, ei)| lattice.edges[from][ei].syllables.len() > 1)
        {
            word_completions.insert(cand.text.clone());
        }
        if !seen.insert(cand.text.clone()) {
            continue;
        }
        found.push((cand, raw, Some(pi)));
    }

    // 第二道防线：只要存在「完全切分成功」的候选，就把含原样字母的混排候选全部丢掉。
    // 用户打出 `nihaoma` 时不该看到 `你好ma`；只有整串都无法切分时（`nihaomx`）
    // 才保留原样字母，让用户看见自己敲了什么。
    let any_clean = found.iter().any(|(_, raw, _)| !raw);
    if any_clean {
        found.retain(|(_, raw, _)| !raw);
    }

    let mut candidates: Vec<Candidate> = Vec::with_capacity(found.len());
    let mut first_syllables: Vec<String> = Vec::new();
    let mut matched = 0usize;
    let mut has_raw = false;
    let mut best_path: Option<usize> = None;
    for (i, (cand, raw, pi)) in found.into_iter().enumerate() {
        // 首选路径决定组字串的音节显示与「已消费」计数
        if i == 0 {
            first_syllables = cand
                .syllables
                .iter()
                .filter_map(|id| syllables::name_of(*id).map(str::to_owned))
                .collect();
            matched = cand.syllable_len;
            has_raw = raw;
            best_path = pi;
        }
        candidates.push(cand);
    }

    // 整串能切分时只沿首选生成前缀；否则从每条混排路径取原样字母之前的中文部分。
    // 例如 wo'e 的候选应是「我、窝、握…」，而不是「我e、窝e、握e」。
    if opts.include_prefixes {
        let prefix_paths: Vec<usize> = if any_clean {
            best_path.into_iter().collect()
        } else {
            (0..paths.len()).collect()
        };
        for pi in prefix_paths {
            if let Some((_, path)) = paths.get(pi) {
                for pc in prefix_candidates(&lattice, path) {
                    if seen.insert(pc.text.clone()) {
                        candidates.push(pc);
                    }
                }
            }
        }
        if has_whole_word {
            // A whole-word path contains no shorter words to extract. Query
            // its dictionary prefixes directly so partial selection remains
            // available without running the automatic sentence search.
            let start = input.len() - input.trim_start_matches('\'').len();
            let end = input.trim_end_matches('\'').len();
            let mut prefixes: Vec<_> = lattice.edges[start]
                .iter()
                .filter(|edge| edge.to < end && matches!(edge.text, EdgeText::Word(_)))
                .collect();
            prefixes.sort_by(|a, b| b.to.cmp(&a.to).then_with(|| b.score.total_cmp(&a.score)));
            for edge in prefixes {
                let Some(mut candidate) = word_candidate(edge, edge.to) else {
                    continue;
                };
                if seen.insert(candidate.text.clone()) {
                    if candidate.syllable_len == 1 {
                        candidate.source = CandidateSource::SingleChar;
                    }
                    candidates.push(candidate);
                }
            }
        }
    }

    if lex.personalize_candidates(&mut candidates) {
        // Keep complete/prefix consumption semantics; within each coverage bucket use
        // the adjusted path score. Without evidence the existing ordering is untouched.
        candidates.sort_by(|a, b| {
            b.consumed
                .cmp(&a.consumed)
                .then_with(|| b.score.total_cmp(&a.score))
                .then_with(|| a.text.cmp(&b.text))
        });
        if let Some(first) = candidates.first() {
            first_syllables = first
                .syllables
                .iter()
                .filter_map(|id| syllables::name_of(*id).map(str::to_owned))
                .collect();
            matched = first.syllable_len;
            has_raw = first.source == CandidateSource::Raw;
        }
    }
    DecodeOutput {
        has_whole_word,
        candidates,
        syllables: first_syllables,
        matched_syllables: matched,
        has_raw,
        word_completions,
    }
}

/// 一条路径上的一步（一个词边或一个原样字母边）。
#[derive(Debug, Clone, PartialEq)]
pub struct PathStep {
    /// 这一步产出的文字（原样字母边就是那个字母）
    pub text: String,
    /// 消费的音节
    pub syllables: Vec<String>,
    /// 消费的输入字符数
    pub consumed: usize,
    /// 词库给出的 logp（原样字母边为 0）
    pub logp: f32,
    /// 这一步的总分（含词边界惩罚）
    pub score: f32,
    pub raw: bool,
}

/// 一条完整路径的逐边分解。
#[derive(Debug, Clone, PartialEq)]
pub struct PathTrace {
    pub score: f32,
    pub text: String,
    pub steps: Vec<PathStep>,
}

/// 逐边分解 top-k 路径。
///
/// 这是排查「为什么 A 排在 B 前面」的唯一可靠手段：只看候选文字和总分时，
/// 相同的文字可能来自完全不同的切分（`你好+吗+世界` vs `你+好吗+世界`），
/// 光看结果永远猜不出原因。`retype-diag --explain` 就是基于它。
pub fn trace(input: &str, lex: &dyn Lexicon, opts: &DecodeOptions) -> Vec<PathTrace> {
    if input.is_empty() {
        return Vec::new();
    }
    let lattice = build_lattice(input, lex, opts);
    let paths = decode_paths(input, &lattice, opts);
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(paths.len());

    for (score, path) in paths.iter() {
        let mut steps = Vec::new();
        let mut text = String::new();
        let mut ok = true;
        for (from, ei) in path {
            let Some(edge) = lattice.edges.get(*from).and_then(|v| v.get(*ei)) else {
                ok = false;
                break;
            };
            let consumed = edge.to.saturating_sub(*from);
            let (step_text, logp, raw) = match &edge.text {
                EdgeText::Word(t) => (t.to_string(), edge.score - opts.word_bonus, false),
                EdgeText::Raw => {
                    let c = bytes.get(*from).map(|b| *b as char).unwrap_or('?');
                    (c.to_string(), 0.0, true)
                }
                EdgeText::Skip => (String::new(), 0.0, false),
            };
            text.push_str(&step_text);
            steps.push(PathStep {
                text: step_text,
                syllables: edge
                    .syllables
                    .iter()
                    .filter_map(|id| syllables::name_of(*id).map(str::to_owned))
                    .collect(),
                consumed,
                logp,
                score: edge.score,
                raw,
            });
        }
        if ok && !text.is_empty() {
            out.push(PathTrace {
                score: *score,
                text,
                steps,
            });
        }
    }
    out
}

/// 列出全部合法切分（调试与单元测试用）。`limit` 防止组合爆炸。
pub fn all_segmentations(input: &str, limit: usize) -> Vec<Vec<String>> {
    let norm = normalize(input);
    let bytes = norm.as_bytes();
    let n = bytes.len();
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut buf: Vec<(usize, Vec<SyllableId>)> = vec![(0, Vec::new())];
    let mut opts_buf: Vec<(usize, SyllableId)> = Vec::new();
    while let Some((pos, ids)) = buf.pop() {
        if out.len() >= limit {
            break;
        }
        if pos >= n {
            let names: Vec<String> = ids
                .iter()
                .filter_map(|id| syllables::name_of(*id).map(str::to_owned))
                .collect();
            if !names.is_empty() && !out.contains(&names) {
                out.push(names);
            }
            continue;
        }
        if bytes[pos] == b'\'' {
            buf.push((pos + 1, ids));
            continue;
        }
        syllable_options(bytes, pos, &mut opts_buf);
        for (len, id) in opts_buf.iter().copied() {
            let mut next = ids.clone();
            next.push(id);
            buf.push((pos + len, next));
        }
    }
    out
}
