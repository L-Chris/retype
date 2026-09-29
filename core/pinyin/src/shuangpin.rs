//! Flypy two-key syllables. The lattice operates on ORIGINAL keystroke offsets,
//! so prefix selection/backspace never confuse expanded pinyin with typed keys.
//! Layout reference: https://flypy.com/ (小鹤双拼); also checked against Rime's Flypy schema.
use crate::{
    decode::{build_lattice_with, decode_lattice},
    syllables, DecodeOptions, DecodeOutput, Lexicon,
};
use retype_types::{Candidate, CandidateSource, SyllableId};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, OnceLock},
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
    let segment = input[..pos]
        .iter()
        .rposition(|b| *b == b'\'')
        .map_or(0, |i| i + 1);
    if (pos - segment) % 2 != 0 {
        return;
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
    let mut output = decode_lattice(&normalized, lattice, opts);
    if normalized
        .rsplit('\'')
        .next()
        .is_some_and(|segment| segment.len() % 2 == 1)
    {
        let mut candidates = incomplete_code_candidates(&normalized, lex, opts);
        let mut raw = Vec::new();
        for candidate in output.candidates {
            if candidate.source == CandidateSource::Raw {
                raw.push(candidate);
            } else {
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
                .partial_cmp(&rank(a))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.consumed.cmp(&a.consumed))
                .then_with(|| a.text.cmp(&b.text))
        });
        let mut seen = HashSet::new();
        candidates.retain(|c| seen.insert(c.text.clone()));
        candidates.extend(raw.into_iter().filter(|c| seen.insert(c.text.clone())));
        output.candidates = candidates;
    }
    output
}

/// Preview dictionary matches while the last Flypy syllable has only its first key.
/// A three-key input such as `mwy` queries phrases for `mei` + `y*`, so `没有`
/// can appear before the user types the fourth key.
fn incomplete_code_candidates(
    input: &str,
    lex: &dyn Lexicon,
    opts: &DecodeOptions,
) -> Vec<Candidate> {
    let Some(&first_key) = input.as_bytes().last() else {
        return Vec::new();
    };
    if !first_key.is_ascii_lowercase() {
        return Vec::new();
    }
    let complete = input[..input.len() - 1].trim_end_matches('\'');
    let prefixes: Vec<Vec<SyllableId>> = if complete.is_empty() {
        vec![Vec::new()]
    } else {
        let decoded = decode(complete, lex, opts);
        let mut seen = HashSet::new();
        decoded
            .candidates
            .into_iter()
            .filter(|c| c.source != CandidateSource::Raw && c.consumed == complete.len())
            .map(|c| c.syllables)
            .filter(|ids| seen.insert(ids.clone()))
            .collect()
    };
    if prefixes.is_empty() {
        return Vec::new();
    }

    let mut final_ids = HashSet::new();
    for (code, ids) in table() {
        if code[0] == first_key {
            final_ids.extend(ids.iter().copied());
        }
    }
    // Keep pronunciations and scores until the top candidates are known. Building
    // display strings for every homophone on each keystroke is needlessly costly.
    let mut matches: HashMap<Arc<str>, (Vec<SyllableId>, f32)> = HashMap::new();
    let mut entries = Vec::new();
    for prefix in prefixes {
        for id in &final_ids {
            let mut syllables = prefix.clone();
            syllables.push(*id);
            entries.clear();
            lex.lookup(&syllables, &mut entries);
            for entry in &entries {
                let score = entry.logp + opts.word_bonus;
                let slot = matches
                    .entry(Arc::clone(&entry.text))
                    .or_insert_with(|| (syllables.clone(), score));
                if score > slot.1 {
                    *slot = (syllables.clone(), score);
                }
            }
        }
    }
    let mut ranked: Vec<_> = matches.into_iter().collect();
    ranked.sort_by(|(a_text, (_, a_score)), (b_text, (_, b_score))| {
        b_score
            .partial_cmp(a_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a_text.cmp(b_text))
    });
    ranked
        .into_iter()
        .take(64)
        .map(|(text, (syllables, score))| Candidate {
            source: if text.chars().count() == 1 {
                CandidateSource::SingleChar
            } else {
                CandidateSource::Local
            },
            comment: syllables
                .iter()
                .filter_map(|id| syllables::name_of(*id))
                .collect::<Vec<_>>()
                .join("'"),
            syllable_len: syllables.len(),
            consumed: input.len(),
            syllables,
            score,
            text: text.to_string(),
        })
        .collect()
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
    fn only_complete_pairs_at_boundaries() {
        let mut out = Vec::new();
        options(b"nihc", 0, &mut out);
        assert_eq!(out, vec![(2, syllables::id_of("ni").unwrap_or_default())]);
        options(b"nihc", 1, &mut out);
        assert!(out.is_empty());
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
