//! Flypy syllables with single-key zero-initial aliases, using ORIGINAL key offsets,
//! so prefix selection/backspace never confuse expanded pinyin with typed keys.
//! Layout reference: https://flypy.com/ (小鹤双拼); also checked against Rime's Flypy schema.
use crate::{
    decode::{build_lattice_with, decode_lattice},
    syllables, DecodeOptions, DecodeOutput, Lexicon,
};
use retype_types::{Candidate, CandidateSource, SyllableId};
use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

pub fn encode(syllable: &str) -> Option<[u8; 2]> {
    let first = *syllable.as_bytes().first()?;
    if matches!(first, b'a' | b'e' | b'o') {
        return match syllable {
            "a" => Some(*b"aa"),
            "e" => Some(*b"ee"),
            "o" => Some(*b"oo"),
            "ang" => Some(*b"ah"),
            "eng" => Some(*b"eg"),
            s if s.len() == 2 => Some([first, s.as_bytes()[1]]),
            _ => None,
        };
    }
    let (initial, final_part) = if let Some(rest) = syllable.strip_prefix("zh") {
        (b'v', rest)
    } else if let Some(rest) = syllable.strip_prefix("ch") {
        (b'i', rest)
    } else if let Some(rest) = syllable.strip_prefix("sh") {
        (b'u', rest)
    } else {
        (first, syllable.get(1..)?)
    };
    let final_key = match final_part {
        "a" => b'a',
        "o" | "uo" => b'o',
        "e" => b'e',
        "i" => b'i',
        "u" => b'u',
        "v" => b'v',
        "iu" => b'q',
        "ei" => b'w',
        "uan" => b'r',
        "ue" | "ve" => b't',
        "un" => b'y',
        "ie" => b'p',
        "ong" | "iong" => b's',
        "ing" | "uai" => b'k',
        "ai" => b'd',
        "en" => b'f',
        "eng" => b'g',
        "iang" | "uang" => b'l',
        "ang" => b'h',
        "ian" => b'm',
        "an" => b'j',
        "ou" => b'z',
        "ia" | "ua" => b'x',
        "iao" => b'n',
        "ao" => b'c',
        "ui" => b'v',
        "in" => b'b',
        _ => return None,
    };
    Some([initial, final_key])
}

fn table() -> &'static HashMap<[u8; 2], Vec<SyllableId>> {
    static TABLE: OnceLock<HashMap<[u8; 2], Vec<SyllableId>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table: HashMap<_, Vec<_>> = HashMap::new();
        for (id, syllable) in syllables::SYLLABLES.iter().enumerate() {
            if let Some(code) = encode(syllable) {
                table.entry(code).or_default().push(id as SyllableId);
            }
            if matches!(*syllable, "ju" | "qu" | "xu" | "yu") {
                table
                    .entry([syllable.as_bytes()[0], b'v'])
                    .or_default()
                    .push(id as SyllableId);
            }
        }
        table
    })
}

fn options(input: &[u8], mut pos: usize, out: &mut Vec<(usize, SyllableId)>) {
    out.clear();
    let start = pos;
    while input.get(pos) == Some(&b'\'') {
        pos += 1;
    }
    // Reachable boundaries come from the lattice, not fixed key-pair parity.
    // Single-key zero initials shift every subsequent two-key syllable.
    let zero_initial = match input.get(pos) {
        Some(b'a') => Some("a"),
        Some(b'e') => Some("e"),
        Some(b'o') => Some("o"),
        _ => None,
    };
    if let Some(id) = zero_initial.and_then(syllables::id_of) {
        out.push((pos - start + 1, id));
    }
    if let (Some(a), Some(b)) = (input.get(pos), input.get(pos + 1)) {
        if let Some(ids) = table().get(&[*a, *b]) {
            out.extend(ids.iter().map(|id| (pos - start + 2, *id)));
        }
    }
}

pub fn decode(input: &str, lex: &dyn Lexicon, opts: &DecodeOptions) -> DecodeOutput {
    let normalized = crate::normalize(input);
    let lattice = build_lattice_with(&normalized, lex, opts, options);
    let has_exact_word = lattice.has_whole_word(&normalized);
    let mut output = decode_lattice(&normalized, lattice, opts, lex);
    output.has_whole_word = has_exact_word;
    if can_preview_tail(normalized.as_bytes()) {
        let preview = build_lattice_with(&normalized, lex, opts, preview_options);
        let has_preview_word = preview.has_whole_word(&normalized);
        let mut candidates: Vec<_> = decode_lattice(&normalized, preview, opts, lex)
            .candidates
            .into_iter()
            .filter(|c| c.source != CandidateSource::Raw)
            .collect();
        if opts.include_prefixes {
            // A single-key vowel may complete a phrase directly, hiding the
            // preceding word from its best path. Keep that prefix selectable.
            let prefix = &normalized[..normalized.len() - 1];
            let lattice = build_lattice_with(prefix, lex, opts, options);
            candidates.extend(
                decode_lattice(prefix, lattice, opts, lex)
                    .candidates
                    .into_iter()
                    .filter(|c| c.source != CandidateSource::Raw),
            );
        }
        let mut exact = Vec::new();
        let mut raw = Vec::new();
        for candidate in output.candidates {
            if candidate.source == CandidateSource::Raw {
                raw.push(candidate);
            } else if has_exact_word && candidate.consumed == normalized.len() {
                // A complete code is more reliable than a tail completion,
                // even when both consume all typed keys.
                exact.push(candidate);
            } else if !has_preview_word || candidate.consumed < normalized.len() {
                // Do not reintroduce automatic compositions from the strict
                // graph when a dictionary word covers the preview graph.
                candidates.push(candidate);
            }
        }
        candidates.sort_by(|a, b| {
            // Compare dictionary evidence per syllable. Whole-phrase log
            // probabilities otherwise lose to a high-frequency single word
            // merely because they cover more of the typed input.
            let rank =
                |candidate: &Candidate| candidate.score / candidate.syllable_len.max(1) as f32;
            rank(b)
                .total_cmp(&rank(a))
                .then_with(|| b.consumed.cmp(&a.consumed))
                .then_with(|| a.text.cmp(&b.text))
        });
        exact.extend(candidates);
        let mut candidates = exact;
        let mut seen = HashSet::new();
        candidates.retain(|c| seen.insert(c.text.clone()));
        candidates.extend(raw.into_iter().filter(|c| seen.insert(c.text.clone())));
        output.candidates = candidates;
        if let Some(first) = output.candidates.first() {
            output.syllables = first
                .syllables
                .iter()
                .filter_map(|id| syllables::name_of(*id).map(str::to_owned))
                .collect();
            output.matched_syllables = first.syllable_len;
            output.has_raw = first.source == CandidateSource::Raw;
        }
    }
    output
}

/// Unfinished syllables use the same dictionary-pruned graph as complete codes.
/// Only the final key can be incomplete; interior consonants remain two-key codes.
fn preview_options(input: &[u8], pos: usize, out: &mut Vec<(usize, SyllableId)>) {
    options(input, pos, out);
    let mut end = pos;
    while input.get(end) == Some(&b'\'') {
        end += 1;
    }
    if end + 1 != input.len() {
        return;
    }
    for (code, ids) in table() {
        if input.get(end) == Some(&code[0]) {
            for &id in ids {
                let option = (input.len() - pos, id);
                if !out.contains(&option) {
                    out.push(option);
                }
            }
        }
    }
    // HashMap iteration order must not affect bounded k-best search ties.
    out.sort_unstable();
}

/// Walk complete syllable boundaries to see whether the last key can be a new
/// syllable. Unlike parity, this also works after single-key zero initials, and
/// avoids constructing a preview lattice for ordinary complete two-key input.
fn can_preview_tail(input: &[u8]) -> bool {
    if input.is_empty() || !input[input.len() - 1].is_ascii_lowercase() {
        return false;
    }
    let mut reachable = vec![false; input.len()];
    reachable[0] = true;
    let mut out = Vec::new();
    for pos in 0..input.len() - 1 {
        if !reachable[pos] {
            continue;
        }
        if input[pos] == b'\'' {
            reachable[pos + 1] = true;
        }
        options(input, pos, &mut out);
        for &(len, _) in &out {
            if pos + len < input.len() {
                reachable[pos + len] = true;
            }
        }
    }
    reachable[input.len() - 1]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_and_zero_initials() {
        for (syllable, code) in [
            ("ni", "ni"),
            ("hao", "hc"),
            ("zhong", "vs"),
            ("guo", "go"),
            ("shuang", "ul"),
            ("pin", "pb"),
            ("xian", "xm"),
            ("a", "aa"),
            ("ai", "ai"),
            ("ang", "ah"),
            ("eng", "eg"),
            ("er", "er"),
            ("nv", "nv"),
            ("lve", "lt"),
        ] {
            assert_eq!(
                encode(syllable).as_ref().map(|s| s.as_slice()),
                Some(code.as_bytes())
            );
        }
    }
    #[test]
    fn variable_length_options_respect_explicit_boundaries() {
        let mut out = Vec::new();
        options(b"nihc", 0, &mut out);
        assert_eq!(out, vec![(2, syllables::id_of("ni").unwrap_or_default())]);
        options(b"edu", 1, &mut out);
        assert_eq!(out, vec![(2, syllables::id_of("du").unwrap_or_default())]);
        options(b"n", 0, &mut out);
        assert!(out.is_empty());
        options(b"ni'hc", 2, &mut out);
        assert_eq!(out, vec![(3, syllables::id_of("hao").unwrap_or_default())]);
        options(b"jv", 0, &mut out);
        assert!(out
            .iter()
            .any(|(_, id)| syllables::name_of(*id) == Some("ju")));
    }
}
