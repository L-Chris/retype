//! Flypy two-key syllables. The lattice operates on ORIGINAL keystroke offsets,
//! so prefix selection/backspace never confuse expanded pinyin with typed keys.
//! Layout reference: https://flypy.com/ (小鹤双拼); also checked against Rime's Flypy schema.
use crate::{
    decode::{build_lattice_with, decode_lattice},
    syllables, DecodeOptions, DecodeOutput, Lexicon,
};
use retype_types::SyllableId;
use std::{collections::HashMap, sync::OnceLock};

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
    decode_lattice(&normalized, lattice, opts)
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
