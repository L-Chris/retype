//! Immutable English vocabulary embedded in the module's file-backed image.
//! No runtime dictionary building or full-vocabulary heap allocations.
#![forbid(unsafe_code)]
use retype_types::{Candidate, CandidateLanguage, CandidateSource, LearningStore};
use std::collections::{BinaryHeap, HashSet};

const DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/english.bin"));
fn u32_at(at: usize) -> usize {
    u32::from_le_bytes([DATA[at], DATA[at + 1], DATA[at + 2], DATA[at + 3]]) as usize
}
fn count() -> usize {
    u32_at(0)
}
fn size() -> usize {
    u32_at(4)
}
fn word(id: usize) -> &'static str {
    let at = 8 + id * 16;
    let arena = 8 + count() * 16 + size() * 8;
    std::str::from_utf8(&DATA[arena + u32_at(at)..arena + u32_at(at) + u32_at(at + 4)])
        .unwrap_or("")
}
fn frequency(id: usize) -> u64 {
    let at = 16 + id * 16;
    u64::from_le_bytes(DATA[at..at + 8].try_into().unwrap_or([0; 8]))
}
fn lower_bound(input: &str) -> usize {
    let (mut lo, mut hi) = (0, count());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if word(mid) < input {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}
fn find(input: &str) -> Option<usize> {
    let i = lower_bound(input);
    (i < count() && word(i) == input).then_some(i)
}
pub fn valid_word(input: &str) -> bool {
    !input.is_empty()
        && input.len() <= 64
        && input.as_bytes()[0].is_ascii_alphabetic()
        && input.as_bytes()[input.len() - 1].is_ascii_alphabetic()
        && input.bytes().all(|b| b.is_ascii_alphabetic() || b == b'\'')
}
fn casing(word: &str, input: &str) -> String {
    if word != "i" && word.eq_ignore_ascii_case(input) {
        return input.to_owned();
    }
    if input.bytes().skip(1).any(|c| c.is_ascii_uppercase())
        && !input.bytes().all(|c| c.is_ascii_uppercase())
        && word.starts_with(&input.to_ascii_lowercase())
    {
        return format!("{input}{}", &word[input.len()..]);
    }
    let letters: Vec<_> = input.bytes().filter(u8::is_ascii_alphabetic).collect();
    if !letters.is_empty() && letters.iter().all(u8::is_ascii_uppercase) {
        return word.to_ascii_uppercase();
    }
    if letters.first().is_some_and(u8::is_ascii_uppercase) {
        let mut output = word.to_owned();
        if let Some(first) = output.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        return output;
    }
    if word == "i" {
        "I".into()
    } else {
        word.into()
    }
}
fn candidate(id: usize, input: &str) -> Candidate {
    let mut c = Candidate::new(casing(word(id), input), CandidateSource::Local);
    c.language = CandidateLanguage::English;
    c.consumed = input.len();
    c.score = (frequency(id) as f64).ln() as f32;
    c
}
/// Top-frequency prefix results via a precompiled range-max tree.
pub fn complete(input: &str, learner: &dyn LearningStore, limit: usize) -> Vec<Candidate> {
    if !valid_word(input) {
        return Vec::new();
    }
    let key = input.to_ascii_lowercase();
    let lo = lower_bound(&key);
    let hi = lower_bound(&format!("{key}{{"));
    let (mut l, mut r) = (lo + size(), hi + size());
    let mut heap = BinaryHeap::new();
    let push = |heap: &mut BinaryHeap<_>, node: usize| {
        let id = u32_at(8 + count() * 16 + node * 4);
        if id < count() {
            heap.push((frequency(id), std::cmp::Reverse(id), node));
        }
    };
    while l < r {
        if l % 2 == 1 {
            push(&mut heap, l);
            l += 1;
        }
        if r % 2 == 1 {
            r -= 1;
            push(&mut heap, r);
        }
        l /= 2;
        r /= 2;
    }
    let mut hits = Vec::new();
    while hits.len() < limit {
        let Some((_, _, node)) = heap.pop() else {
            break;
        };
        if node >= size() {
            hits.push(candidate(node - size(), input));
        } else {
            push(&mut heap, node * 2);
            push(&mut heap, node * 2 + 1);
        }
    }
    // Empirical personal counts form a smoothed distribution, rather than a
    // fixed per-selection bonus. Keep the vocabulary prior for known words.
    let total: f64 = hits
        .iter()
        .map(|c| f64::from(c.score).exp())
        .sum::<f64>()
        .max(1.);
    for c in &mut hits {
        c.score -= total.ln() as f32;
    }
    for (text, uses) in learner.english_words(&key, limit) {
        let normalized = text.to_ascii_lowercase();
        let prior = find(&normalized).map_or(0., |id| frequency(id) as f64);
        let score = (prior / total + uses as f64).ln() as f32;
        let text = casing(&text, input);
        if let Some(existing) = hits.iter_mut().find(|c| c.text == text) {
            existing.score = score;
            existing.source = CandidateSource::User;
        } else {
            let mut c = Candidate::new(text, CandidateSource::User);
            c.language = CandidateLanguage::English;
            c.consumed = input.len();
            c.score = score;
            hits.push(c);
        }
    }
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.text.cmp(&b.text))
    });
    hits.truncate(limit);
    hits
}

/// A small mixed-language stream, exact spelling before frequency-ranked completions.
/// Exact matches are looked up separately so a high-frequency completion cannot hide one.
pub fn mixed(input: &str, learner: &dyn LearningStore, limit: usize) -> Vec<Candidate> {
    if input.len() < 2 || limit == 0 || !valid_word(input) {
        return Vec::new();
    }
    let mut hits = complete(input, learner, limit);
    if let Some(id) = find(&input.to_ascii_lowercase()) {
        if !hits.iter().any(|c| c.text.eq_ignore_ascii_case(input)) {
            hits.push(candidate(id, input));
        }
    }
    hits.sort_by_key(|c| !c.text.eq_ignore_ascii_case(input));
    hits.truncate(limit);
    hits
}
/// One-edit spelling suggestions (insertion/deletion/substitution/transposition).
/// Called on the background worker, never while holding the kernel lock.
pub fn correct(input: &str, limit: usize) -> Vec<Candidate> {
    if !valid_word(input) || input.len() < 3 {
        return Vec::new();
    }
    let key = input.to_ascii_lowercase();
    let mut ids = HashSet::new();
    let mut add = |s: &str| {
        if s != key {
            if let Some(id) = find(s) {
                ids.insert(id);
            }
        }
    };
    for i in 0..key.len() {
        add(&format!("{}{}", &key[..i], &key[i + 1..]));
        if i + 1 < key.len() {
            let mut swapped = key.as_bytes().to_vec();
            swapped.swap(i, i + 1);
            if let Ok(s) = std::str::from_utf8(&swapped) {
                add(s);
            }
        }
        for c in b'a'..=b'z' {
            add(&format!("{}{}{}", &key[..i], c as char, &key[i + 1..]));
        }
    }
    for i in 0..=key.len() {
        for c in b'a'..=b'z' {
            add(&format!("{}{}{}", &key[..i], c as char, &key[i..]));
        }
    }
    let mut ids: Vec<_> = ids.into_iter().collect();
    ids.sort_by(|a, b| frequency(*b).cmp(&frequency(*a)).then_with(|| a.cmp(b)));
    ids.into_iter()
        .take(limit)
        .map(|id| {
            let mut c = candidate(id, input);
            c.comment = "spelling".into();
            c
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Empty;
    impl LearningStore for Empty {
        fn record(&self, _: retype_types::LearningEvent) {}
    }
    #[test]
    fn mixed_stream_keeps_exact_spelling_case_and_personal_words() {
        for input in ["an", "hello", "Hello", "HELLO"] {
            let words = mixed(input, &Empty, 2);
            assert!(words[0].text.eq_ignore_ascii_case(input));
            assert_eq!(words[0].language, CandidateLanguage::English);
            assert!(words.iter().all(|c| c.consumed == input.len()));
        }
        assert_eq!(mixed("Hello", &Empty, 2)[0].text, "Hello");
        struct Personal;
        impl LearningStore for Personal {
            fn record(&self, _: retype_types::LearningEvent) {}
            fn english_words(&self, prefix: &str, _: usize) -> Vec<(String, u64)> {
                if "retypetestword".starts_with(prefix) {
                    vec![("retypetestword".into(), 10)]
                } else {
                    Vec::new()
                }
            }
        }
        assert_eq!(
            mixed("retypetestword", &Personal, 2)[0].source,
            CandidateSource::User
        );
        assert!(mixed("ni'hao", &Empty, 2).is_empty());
    }
    #[test]
    fn vocabulary_and_range_top_k() {
        assert!(count() > 80_000);
        for prefix in ["a", "hel", "transl", "don't", "z"] {
            let actual = complete(prefix, &Empty, 12);
            let mut expected: Vec<_> = (0..count())
                .filter(|i| word(*i).starts_with(prefix))
                .collect();
            expected.sort_by(|a, b| frequency(*b).cmp(&frequency(*a)).then_with(|| a.cmp(b)));
            assert_eq!(
                actual
                    .iter()
                    .map(|c| c.text.to_lowercase())
                    .collect::<Vec<_>>(),
                expected
                    .into_iter()
                    .take(12)
                    .map(|id| word(id).to_owned())
                    .collect::<Vec<_>>()
            );
        }
    }
    #[test]
    fn spelling_and_case() {
        assert!(correct("hellp", 12).iter().any(|c| c.text == "hello"));
        let suggestions = correct("teh", 12);
        assert!(
            suggestions.iter().any(|c| c.text == "the"),
            "suggestions={suggestions:?}, the={:?}",
            find("the")
        );
        assert!(correct("hello", 12).iter().all(|c| c.text != "hello"));
        assert!(complete("HEL", &Empty, 12)
            .iter()
            .any(|c| c.text == "HELLO"));
        assert!(complete("Hel", &Empty, 12)
            .iter()
            .any(|c| c.text == "Hello"));
        assert!(!valid_word("a@b"));
    }
}
