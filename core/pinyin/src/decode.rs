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
use std::sync::Arc;

/// 解码参数。
#[derive(Debug, Clone)]
pub struct DecodeOptions {
    /// 保留几条完整路径（k-best）
    pub k: usize,
    /// 候选窗每页条数
    pub page_size: usize,
    /// 每个词的边界惩罚。越大越倾向长词（抑制「你+好」压过「你好」）。
    pub word_penalty: f32,
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
            k: 8,
            page_size: 9,
            word_penalty: 0.8,
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
    let bytes = input.as_bytes();
    let n = bytes.len();
    let mut edges: Vec<Vec<Edge>> = vec![Vec::new(); n + 1];
    let mut entries: Vec<LexEntry> = Vec::new();
    let mut syl_opts: Vec<(usize, SyllableId)> = Vec::new();
    let mut stack: Vec<(usize, Vec<SyllableId>)> = Vec::new();

    for i in 0..n {
        let is_sep = bytes[i] == b'\'';
        // 兜底边：任何位置都能消费一个字符，保证 DP 不会走进死胡同
        edges[i].push(Edge {
            to: i + 1,
            text: if is_sep {
                EdgeText::Skip
            } else {
                EdgeText::Raw
            },
            syllables: Vec::new(),
            score: if is_sep { 0.0 } else { -opts.raw_penalty },
        });
        if is_sep {
            continue;
        }

        stack.clear();
        stack.push((i, Vec::new()));
        while let Some((pos, syls)) = stack.pop() {
            syllable_options(bytes, pos, &mut syl_opts);
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
                        text: EdgeText::Word(e.text.clone()),
                        syllables: next.clone(),
                        score: e.logp - opts.word_penalty,
                    });
                }
                if next.len() < opts.max_word_syllables && npos < n {
                    stack.push((npos, next));
                }
            }
        }
    }

    Lattice { n, edges }
}

#[derive(Debug, Clone, Copy)]
struct Beam {
    score: f32,
    /// `u32::MAX` 表示起点
    prev: u32,
    prev_slot: u32,
    edge: u32,
}

const NO_PREV: u32 = u32::MAX;

fn insert_beam(beams: &mut Vec<Beam>, beam: Beam, k: usize) {
    if k == 0 {
        return;
    }
    // 降序插入：找到第一个分数严格更小的位置。k 很小（默认 8），线性扫描比
    // 浮点 binary_search 更快也更直观。
    let mut idx = beams.len();
    for (i, b) in beams.iter().enumerate() {
        if b.score < beam.score {
            idx = i;
            break;
        }
    }
    if idx >= k {
        return;
    }
    beams.insert(idx, beam);
    beams.truncate(k);
}

/// k-best DP。返回按分数降序的路径，每条路径是 `[(起点位置, 边下标)]`。
fn kbest(lattice: &Lattice, k: usize) -> Vec<(f32, Vec<(usize, usize)>)> {
    let n = lattice.n;
    let mut dp: Vec<Vec<Beam>> = vec![Vec::new(); n + 1];
    dp[0].push(Beam {
        score: 0.0,
        prev: NO_PREV,
        prev_slot: 0,
        edge: 0,
    });

    for i in 0..n {
        if dp[i].is_empty() {
            continue;
        }
        // 克隆当前层：边只会指向 > i 的位置，所以不会自我干扰
        let cur = dp[i].clone();
        for (slot, beam) in cur.iter().enumerate() {
            for (ei, e) in lattice.edges[i].iter().enumerate() {
                insert_beam(
                    &mut dp[e.to],
                    Beam {
                        score: beam.score + e.score,
                        prev: i as u32,
                        prev_slot: slot as u32,
                        edge: ei as u32,
                    },
                    k,
                );
            }
        }
    }

    let mut paths = Vec::new();
    for (slot, beam) in dp[n].iter().enumerate() {
        let mut rev: Vec<(usize, usize)> = Vec::new();
        let mut cur_pos = n;
        let mut cur_slot = slot;
        // 路径长度上界 = 字符数，超过说明状态异常，直接止损
        let mut guard = n + 2;
        while guard > 0 {
            guard -= 1;
            let Some(b) = dp.get(cur_pos).and_then(|v| v.get(cur_slot)) else {
                break;
            };
            if b.prev == NO_PREV {
                break;
            }
            rev.push((b.prev as usize, b.edge as usize));
            cur_pos = b.prev as usize;
            cur_slot = b.prev_slot as usize;
        }
        rev.reverse();
        paths.push((beam.score, rev));
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
    let paths = kbest(&lattice, opts.k.max(1));
    let bytes = input.as_bytes();

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut first_syllables: Vec<String> = Vec::new();
    let mut matched = 0usize;
    let mut has_raw = false;

    for (score, path) in paths.iter() {
        let Some((cand, raw)) = path_to_candidate(bytes, &lattice, path, *score) else {
            continue;
        };
        if seen.iter().any(|s| *s == cand.text) {
            continue;
        }
        // 首选路径决定组字串的音节显示与「已消费」计数
        if candidates.is_empty() {
            first_syllables = cand
                .syllables
                .iter()
                .filter_map(|id| syllables::name_of(*id).map(str::to_owned))
                .collect();
            matched = cand.syllable_len;
            has_raw = raw;
        }
        seen.push(cand.text.clone());
        candidates.push(cand);
    }

    if opts.include_prefixes {
        if let Some((_, best)) = paths.first() {
            for pc in prefix_candidates(&lattice, best) {
                if !seen.iter().any(|s| *s == pc.text) {
                    seen.push(pc.text.clone());
                    candidates.push(pc);
                }
            }
        }
    }

    DecodeOutput {
        candidates,
        syllables: first_syllables,
        matched_syllables: matched,
        has_raw,
    }
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
