//! 词格构建 + k-best Viterbi 解码（首刷的算法核心）。
//!
//! 设计要点（对应 ARCHITECTURE.md §2）：
//! - **音节边界歧义由词库解决，不由切分器猜**：切分器把所有合法切分铺成一张词格，
//!   Viterbi 结合词频挑最优路径。`xian` / `jian` / `shanghai` 这类歧义天然被覆盖。
//! - 所有方法都是纯内存计算，必须在输入线程上同步完成（P1），预算 < 5ms。
//! - 词格扩展靠 [`Lexicon::has_prefix`] 剪枝，否则组合数会随输入长度指数爆炸。

use crate::lexicon::{LexEntry, Lexicon};
use crate::syllables;
use retype_types::{Candidate, CandidateSource, SyllableId};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

/// 解码参数。
#[derive(Debug, Clone)]
pub struct DecodeOptions {
    /// 保留几条完整路径（k-best）
    pub k: usize,
    /// 候选窗每页条数
    pub page_size: usize,
    /// 每个词的加分（注意是**加**不是减）。
    ///
    /// unigram 模型下 `logp(w) = ln(freq_w / T)`，一条 k 词路径的总分是
    /// `Σ ln f_i − k·ln T`：**每多切一个词就白扣一次 ln T（≈17.9）**。
    /// 这是模型的固有偏差，不是语言事实 —— 它会让「妳好吗」这种词频≈3 的
    /// 垃圾长词条（logp −16.8）压过「你好(−11.3) + 吗(−7.9)」。
    ///
    /// `word_bonus` 就是这个偏差的部分补偿。理论上界是 `ln T`（完全补偿，
    /// 等价于假设任意词都能自由相接，结果会退化成全单字）；下界是 0（现状，
    /// 会过度合并）。合理区间由不等式给出，见 `word_bonus_bounds` 测试。
    /// M2 引入 bigram/上下文模型后，这一项应退化为一个很小的微调量。
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
            max_word_syllables: 5,
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
    let bytes = input.as_bytes();
    let n = bytes.len();
    let mut edges: Vec<Vec<Edge>> = vec![Vec::new(); n + 1];
    let mut entries: Vec<LexEntry> = Vec::new();
    let mut syl_opts: Vec<(usize, SyllableId)> = Vec::new();
    let mut stack: Vec<(usize, Vec<SyllableId>)> = Vec::new();

    for i in 0..n {
        if bytes[i] == b'\'' {
            // 显式分隔符：消费掉但不产出文字，也不参与切分
            edges[i].push(Edge {
                to: i + 1,
                text: EdgeText::Skip,
                syllables: Vec::new(),
                score: 0.0,
            });
            continue;
        }

        stack.clear();
        stack.push((i, Vec::new()));
        while let Some((pos, syls)) = stack.pop() {
            options(bytes, pos, &mut syl_opts);
            for (len, id) in syl_opts.iter().copied() {
                let npos = pos + len;
                let mut next = syls.clone();
                next.push(id);
                if !lex.has_prefix(&next) {
                    continue;
                }
                entries.clear();
                lex.lookup(&next, &mut entries);
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
    }

    Lattice { n, edges }
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
                insert_node(
                    &mut dp[e.to],
                    Rc::new(Node {
                        score: beam.score + e.score,
                        from: i as u32,
                        edge: ei as u32,
                        prev: Some(Rc::clone(beam)),
                    }),
                    k,
                );
            }
        }
    }

    let mut paths = Vec::new();
    for node in dp[n].iter() {
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

/// 解码结果。
#[derive(Debug, Clone, Default)]
pub struct DecodeOutput {
    /// 已排序去重的候选（完整句候选在前，前缀词候选在后）
    pub candidates: Vec<Candidate>,
    /// 首选路径的音节切分，用于显示 `ni'hao'ma`
    pub syllables: Vec<String>,
    /// 首选路径消费掉的音节数
    pub matched_syllables: usize,
    /// 首选路径是否含未能匹配的字母（true 表示还在打无效串）
    pub has_raw: bool,
}

fn comment_of(syllables: &[SyllableId]) -> String {
    syllables
        .iter()
        .filter_map(|id| syllables::name_of(*id))
        .collect::<Vec<_>>()
        .join("'")
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
            text,
            comment,
            source: CandidateSource::Local,
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
    for (from, ei) in path {
        let Some(edge) = lattice.edges.get(*from).and_then(|v| v.get(*ei)) else {
            break;
        };
        consumed += edge.to.saturating_sub(*from);
        match &edge.text {
            EdgeText::Word(t) => {
                text.push_str(t);
                ids.extend_from_slice(&edge.syllables);
            }
            // 一旦遇到原样字母，后面的前缀就没有意义了
            EdgeText::Raw | EdgeText::Skip => break,
        }
        out.push(Candidate {
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
            score: 0.0,
        });
    }
    // 最后一个前缀 == 完整路径，已由 k-best 产出，去掉
    out.pop();
    out
}

/// 把归一化后的输入解码成候选列表。
pub fn decode(input: &str, lex: &dyn Lexicon, opts: &DecodeOptions) -> DecodeOutput {
    if input.is_empty() {
        return DecodeOutput::default();
    }
    let lattice = build_lattice(input, lex, opts);
    decode_lattice(input, lattice, opts)
}

pub(crate) fn decode_lattice(input: &str, lattice: Lattice, opts: &DecodeOptions) -> DecodeOutput {
    let paths = kbest(&lattice, opts.k.max(1));
    let bytes = input.as_bytes();

    let mut found: Vec<(Candidate, bool, usize)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for (pi, (score, path)) in paths.iter().enumerate() {
        let Some((cand, raw)) = path_to_candidate(bytes, &lattice, path, *score) else {
            continue;
        };
        if !seen.insert(cand.text.clone()) {
            continue;
        }
        found.push((cand, raw, pi));
    }

    // 第二道防线：只要存在「完全切分成功」的候选，就把含原样字母的混排候选全部丢掉。
    // 用户打出 `nihaoma` 时不该看到 `你好ma`；只有整串都无法切分时（`nihaomx`）
    // 才保留原样字母，让用户看见自己敲了什么。
    let any_clean = found.iter().any(|(_, raw, _)| !raw);
    if any_clean {
        found.retain(|(_, raw, _)| !raw);
    }

    let mut candidates: Vec<Candidate> = Vec::new();
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
            best_path = Some(pi);
        }
        candidates.push(cand);
    }

    // 前缀候选必须沿**首选（且干净）**的那条路径切，否则会切出含原样字母的片段
    if opts.include_prefixes {
        if let Some(pi) = best_path.and_then(|i| paths.get(i)) {
            for pc in prefix_candidates(&lattice, &pi.1) {
                if seen.insert(pc.text.clone()) {
                    candidates.push(pc);
                }
            }
        }
    }

    // k-best keeps combined paths bounded. Preserve every direct dictionary match,
    // including rare homophones and low-frequency phrases, so paging can still reach
    // words outside the beam without expanding the combinatorial path search.
    let mut exact_matches: Vec<&Edge> = lattice.edges[0]
        .iter()
        .filter(|e| e.to == lattice.n && !e.syllables.is_empty())
        .filter(|e| matches!(e.text, EdgeText::Word(_)))
        .collect();
    exact_matches.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for edge in exact_matches {
        let EdgeText::Word(text) = &edge.text else {
            continue;
        };
        if seen.insert(text.to_string()) {
            candidates.push(Candidate {
                text: text.to_string(),
                comment: comment_of(&edge.syllables),
                source: if text.chars().count() == 1 {
                    CandidateSource::SingleChar
                } else {
                    CandidateSource::Local
                },
                syllable_len: edge.syllables.len(),
                consumed: lattice.n,
                syllables: edge.syllables.clone(),
                score: edge.score,
            });
        }
    }

    DecodeOutput {
        candidates,
        syllables: first_syllables,
        matched_syllables: matched,
        has_raw,
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
    let paths = kbest(&lattice, opts.k.max(1));
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
