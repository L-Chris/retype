//! Numeric input measurements and streaming English word boundaries, shared by adapters.
//! Only character classes are retained, never the text. This tailors Unicode's
//! word-boundary conventions to English: Latin letters/digits, internal apostrophes,
//! and combining marks; hyphens and other punctuation separate words.
#[derive(Clone, Debug, Default)]
pub struct WordCounter {
    tail: Vec<u8>,
}
impl WordCounter {
    /// Restore the numeric-only checkpoint used across Android JNI calls.
    pub fn restore(tail: &[u8]) -> Option<Self> {
        (tail.len() <= 4096 && tail.iter().all(|v| *v <= 3)).then(|| Self {
            tail: tail.to_vec(),
        })
    }
    pub fn checkpoint(&self) -> &[u8] {
        &self.tail
    }
    pub fn feed(&mut self, text: &str) -> u32 {
        text.chars()
            .map(|ch| self.feed_char(ch))
            .fold(0, u32::saturating_add)
    }
    pub fn feed_char(&mut self, ch: char) -> u32 {
        let class = if is_latin_letter(ch) {
            Some(1)
        } else if ch.is_ascii_digit() {
            Some(0)
        } else if matches!(ch, '\'' | '\u{2019}') && self.tail.last().is_some_and(|v| *v != 2) {
            Some(2)
        } else if matches!(ch as u32, 0x0300..=0x036F) && !self.tail.is_empty() {
            Some(3)
        } else {
            None
        };
        if let Some(class) = class {
            // Bound even a very long uninterrupted token without creating extra words.
            if self.tail.len() < 4096 {
                self.tail.push(class);
            } else if class == 1 && !self.tail.contains(&1) {
                self.tail[0] = 1;
            }
        } else {
            return self.finish();
        }
        0
    }
    pub fn backspace(&mut self) {
        self.tail.pop();
    }
    pub fn finish(&mut self) -> u32 {
        let words = u32::from(self.tail.contains(&1));
        self.tail.clear();
        words
    }
}
fn is_latin_letter(ch: char) -> bool {
    ch.is_alphabetic() && matches!(ch as u32, 0x0041..=0x007A | 0x00C0..=0x024F | 0x1E00..=0x1EFF)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_and_numeric_tokens() {
        let mut counter = WordCounter::default();
        assert_eq!(
            counter.feed("Hello world don't I’m hello-world GPT4 123 🙂 中文 café cafe\u{301} "),
            9
        );
        assert_eq!(counter.finish(), 0);
    }
    #[test]
    fn chunks_backspace_and_numeric_checkpoint() {
        let mut counter = WordCounter::default();
        assert_eq!(counter.feed("hel"), 0);
        counter = WordCounter::restore(counter.checkpoint()).unwrap_or_default();
        counter.backspace();
        assert_eq!(counter.feed("llo "), 1);
        assert_eq!(counter.feed("ab"), 0);
        counter.backspace();
        counter.backspace();
        assert_eq!(counter.finish(), 0);
        assert_eq!(counter.feed("don’"), 0);
        assert_eq!(counter.feed("t "), 1);
        assert_eq!(counter.feed("unfinished"), 0);
        assert_eq!(counter.finish(), 1);
        assert_eq!(counter.finish(), 0);
        assert!(WordCounter::restore(&[4]).is_none());
    }
}

/// Numeric-only measurements. No input text or document identity is retained.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct LengthCounts {
    pub inputs: u64,
    pub corrected: u64,
    pub units: u64,
}
/// Collection buckets: exact lengths 1–19, followed by >=20.
pub const LENGTH_BUCKETS: usize = 20;
/// Presentation only; no grouped counts are persisted.
pub const LENGTH_LABELS: [&str; 6] = ["1", "2", "3", "4", "5–10", "11+"];
pub fn length_index(n: u64) -> usize {
    n.clamp(1, 20) as usize - 1
}
pub fn display_index(n: u64) -> usize {
    match n {
        0..=1 => 0,
        2 => 1,
        3 => 2,
        4 => 3,
        5..=10 => 4,
        _ => 5,
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct InputStats {
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "empty_lengths"))]
    pub chinese: [LengthCounts; LENGTH_BUCKETS],
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "empty_lengths"))]
    pub english: [LengthCounts; LENGTH_BUCKETS],
    pub cancelled: u64,
    pub spelling_deletions: u64,
}
#[cfg(feature = "serde")]
fn empty_lengths(rows: &[LengthCounts; LENGTH_BUCKETS]) -> bool {
    rows.iter().all(|v| *v == LengthCounts::default())
}
impl InputStats {
    pub fn grouped(&self, chinese: bool) -> [LengthCounts; 6] {
        let mut grouped = [LengthCounts::default(); 6];
        for (i, row) in (if chinese {
            &self.chinese
        } else {
            &self.english
        })
        .iter()
        .enumerate()
        {
            let target = &mut grouped[display_index(i as u64 + 1)];
            target.inputs = target.inputs.saturating_add(row.inputs);
            target.corrected = target.corrected.saturating_add(row.corrected);
            target.units = target.units.saturating_add(row.units);
        }
        grouped
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
    pub fn record(&mut self, chinese: bool, units: u64, corrected: bool, deletions: u64) {
        if units == 0 {
            return;
        }
        let row = &mut if chinese {
            &mut self.chinese
        } else {
            &mut self.english
        }[length_index(units)];
        row.inputs = row.inputs.saturating_add(1);
        row.corrected = row.corrected.saturating_add(u64::from(corrected));
        row.units = row.units.saturating_add(units);
        self.spelling_deletions = self.spelling_deletions.saturating_add(deletions);
    }
    pub fn total(&self, chinese: bool) -> LengthCounts {
        let mut result = LengthCounts::default();
        for c in if chinese {
            &self.chinese
        } else {
            &self.english
        } {
            result.inputs = result.inputs.saturating_add(c.inputs);
            result.corrected = result.corrected.saturating_add(c.corrected);
            result.units = result.units.saturating_add(c.units);
        }
        result
    }
    pub fn add(&mut self, other: &Self) {
        self.combine(other, u64::saturating_add);
    }
    pub fn merge_max(&mut self, other: &Self) {
        self.combine(other, u64::max);
    }
    /// Fixed numeric JNI format; contains no text or editor identity.
    pub fn from_values(values: &[u64]) -> Option<Self> {
        if values.len() != LENGTH_BUCKETS * 6 + 2 {
            return None;
        }
        let mut input = Self::default();
        for (row, values) in input
            .chinese
            .iter_mut()
            .chain(&mut input.english)
            .zip(values.as_chunks::<3>().0)
        {
            *row = LengthCounts {
                inputs: values[0],
                corrected: values[1],
                units: values[2],
            };
        }
        input.cancelled = values[LENGTH_BUCKETS * 6];
        input.spelling_deletions = values[LENGTH_BUCKETS * 6 + 1];
        Some(input)
    }
    pub fn values(&self) -> impl Iterator<Item = u64> + '_ {
        self.chinese
            .iter()
            .chain(&self.english)
            .flat_map(|v| [v.inputs, v.corrected, v.units])
            .chain([self.cancelled, self.spelling_deletions])
    }
    fn combine(&mut self, other: &Self, f: impl Fn(u64, u64) -> u64) {
        for (a, b) in self
            .chinese
            .iter_mut()
            .zip(&other.chinese)
            .chain(self.english.iter_mut().zip(&other.english))
        {
            a.inputs = f(a.inputs, b.inputs);
            a.corrected = f(a.corrected, b.corrected);
            a.units = f(a.units, b.units);
        }
        self.cancelled = f(self.cancelled, other.cancelled);
        self.spelling_deletions = f(self.spelling_deletions, other.spelling_deletions);
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct Totals {
    pub chinese: u64,
    pub english: u64,
    pub chinese_ms: u64,
    pub english_ms: u64,
    pub english_words: u64,
    pub english_word_ms: u64,
    #[cfg_attr(
        feature = "serde",
        serde(rename = "input_detail", skip_serializing_if = "InputStats::is_empty")
    )]
    pub input: InputStats,
}
impl Totals {
    fn combine(&mut self, other: &Self, f: impl Fn(u64, u64) -> u64) {
        self.chinese = f(self.chinese, other.chinese);
        self.english = f(self.english, other.english);
        self.chinese_ms = f(self.chinese_ms, other.chinese_ms);
        self.english_ms = f(self.english_ms, other.english_ms);
        self.english_words = f(self.english_words, other.english_words);
        self.english_word_ms = f(self.english_word_ms, other.english_word_ms);
        self.input.combine(&other.input, f);
    }
    pub fn add(&mut self, other: &Self) {
        self.combine(other, u64::saturating_add);
    }
    pub fn merge_max(&mut self, other: &Self) {
        self.combine(other, u64::max);
    }
    pub fn subtract(&mut self, other: &Self) {
        self.combine(other, u64::saturating_sub);
    }
    pub fn total(self) -> u64 {
        self.chinese.saturating_add(self.english)
    }
    pub fn chinese_speed(self) -> Option<u64> {
        speed(self.chinese, self.chinese_ms, 10)
    }
    pub fn english_speed(self) -> Option<u64> {
        speed(self.english_words, self.english_word_ms, 5)
    }
}
/// Typing characters exclude digits and punctuation on every platform.
pub fn character_counts(text: &str) -> (u64, u64) {
    text.chars().fold((0, 0), |(zh, en), ch| {
        (
            zh + u64::from(is_han(ch)),
            en + u64::from(ch.is_ascii_alphabetic()),
        )
    })
}
/// Allocate one committed composition's active time using the shared character policy.
pub fn commit_counts(text: &str, elapsed: u64) -> Totals {
    let (chinese, english) = character_counts(text);
    let chinese_ms = ((u128::from(elapsed) * u128::from(chinese))
        / u128::from((chinese + english).max(1))) as u64;
    let english_ms = if english > 0 {
        elapsed.saturating_sub(chinese_ms)
    } else {
        0
    };
    Totals {
        chinese,
        english,
        chinese_ms,
        english_ms,
        english_word_ms: english_ms,
        ..Default::default()
    }
}
pub fn speed(count: u64, ms: u64, minimum: u64) -> Option<u64> {
    (count >= minimum && ms >= 10_000).then(|| (count as f64 * 60_000.0 / ms as f64).round() as u64)
}
/// A round survives partial candidate choices; reopening a chosen part is not a
/// spelling deletion. The platform reports actual buffer deletions explicitly.
#[derive(Clone, Copy, Debug, Default)]
pub struct Attempt {
    pub active: bool,
    pub deletions: u64,
}
impl Attempt {
    pub fn edit(&mut self, before: bool, after: bool, deleted: bool) {
        self.active |= before || after;
        if self.active && deleted {
            self.deletions = self.deletions.saturating_add(1);
        }
    }
    pub fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}
#[cfg(feature = "serde")]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LogRecord {
    pub second: u64,
    #[serde(default)]
    pub version: String,
    pub counts: Totals,
}
#[cfg(feature = "serde")]
impl LogRecord {
    pub fn parse(line: &str) -> Option<Self> {
        let v: Vec<u64> = line
            .trim()
            .split(',')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        if v.len() != 5 && v.len() != 7 {
            return None;
        }
        Some(Self {
            second: v[0],
            version: String::new(),
            counts: Totals {
                chinese: v[1],
                english: v[2],
                chinese_ms: v[3],
                english_ms: v[4],
                english_words: v.get(5).copied().unwrap_or(0),
                english_word_ms: v.get(6).copied().unwrap_or(0),
                ..Default::default()
            },
        })
    }
}

/// Shared count-only collector; platforms acknowledge successful editor writes
/// before persisting returned deltas. No UI, document reads, or I/O occurs here.
#[derive(Clone, Debug, Default)]
pub struct InputTracker {
    attempt: Attempt,
    direct_words: WordCounter,
    direct_attempt: Attempt,
}
impl InputTracker {
    pub fn edit(&mut self, before: (bool, usize), after: (bool, usize), backspace: bool) {
        self.attempt
            .edit(before.0, after.0, backspace && before.1 > after.1);
    }
    pub fn commit(&mut self, text: &str) -> InputStats {
        let mut result = InputStats::default();
        let chinese = text.chars().filter(|c| is_han(*c)).count() as u64;
        if self.attempt.active && text.chars().any(|c| is_han(c) || is_latin_letter(c)) {
            let attempt = self.attempt.take();
            result.record(true, chinese, attempt.deletions > 0, attempt.deletions);
            let mut counter = WordCounter::default();
            let words = counter.feed(text) + counter.finish();
            result.record(
                false,
                words.into(),
                attempt.deletions > 0,
                if chinese == 0 { attempt.deletions } else { 0 },
            );
        } else if chinese == 0 {
            for ch in text.chars() {
                if self.direct_words.feed_char(ch) > 0 {
                    let a = self.direct_attempt.take();
                    result.record(false, 1, a.deletions > 0, a.deletions);
                }
                if self.direct_words.checkpoint().contains(&1) {
                    self.direct_attempt.active = true;
                } else if self.direct_words.checkpoint().is_empty() {
                    self.direct_attempt = Attempt::default();
                }
            }
        }
        result
    }
    pub fn boundary(&mut self, cancel: bool) -> InputStats {
        let mut result = InputStats::default();
        if cancel && self.attempt.take().active {
            result.cancelled = 1;
        }
        let words = self.direct_words.finish();
        let a = self.direct_attempt.take();
        result.record(false, words.into(), a.deletions > 0, a.deletions);
        result
    }
    pub fn backspace(&mut self) -> InputStats {
        let mut result = InputStats::default();
        if self.direct_words.checkpoint().contains(&1) {
            self.direct_attempt.deletions = self.direct_attempt.deletions.saturating_add(1);
        }
        self.direct_words.backspace();
        if self.direct_words.checkpoint().is_empty() && self.direct_attempt.take().active {
            result.cancelled = 1;
        }
        result
    }
    pub fn discard(&mut self) {
        self.attempt = Attempt::default();
    }
}
pub fn is_han(ch: char) -> bool {
    matches!(ch as u32,0x3400..=0x4DBF|0x4E00..=0x9FFF|0xF900..=0xFAFF|0x20000..=0x2A6DF|0x2A700..=0x2B73F|0x2B740..=0x2B81F|0x2B820..=0x2CEAF|0x2CEB0..=0x2EBEF|0x30000..=0x323AF)
}

#[cfg(test)]
mod input_tests {
    use super::*;
    #[test]
    fn exact_buckets_reaggregate_at_four_five_ten_eleven_and_keep_tail_mean() {
        let mut input = InputStats::default();
        for n in [1, 2, 3, 4, 5, 9, 10, 11, 19, 20, 27] {
            input.record(true, n, n % 2 == 0, 0);
        }
        assert_eq!(input.chinese[9].inputs, 1);
        assert_eq!(input.chinese[10].inputs, 1);
        assert_eq!(input.chinese[19].inputs, 2);
        assert_eq!(input.chinese[19].units, 47);
        let grouped = input.grouped(true);
        assert_eq!(grouped.map(|r| r.inputs), [1, 1, 1, 1, 3, 4]);
        assert_eq!(grouped[4].units, 24);
        assert_eq!(grouped[5].units, 77);
        assert_eq!(input.total(true).inputs, 11);
        assert_eq!(input.values().count(), 122);
    }
    #[test]
    fn words_and_partial_selections_and_failures_share_one_round() {
        let mut tracker = InputTracker::default();
        tracker.edit((false, 0), (true, 6), false);
        tracker.edit((true, 6), (true, 4), false);
        tracker.edit((true, 0), (true, 4), true);
        assert_eq!(tracker.commit("你好").total(true).corrected, 0);
        tracker.edit((false, 0), (true, 6), false);
        tracker.edit((true, 6), (true, 5), true);
        assert_eq!(tracker.commit("hello world").english[1].corrected, 1);
        assert!(tracker.boundary(false).is_empty());
        tracker.edit((false, 0), (true, 1), false);
        assert_eq!(tracker.boundary(true).cancelled, 1);
        assert!(tracker.boundary(true).is_empty());
        tracker.edit((false, 0), (true, 6), false);
        tracker.discard();
        assert!(tracker.boundary(true).is_empty());
        assert!(tracker.commit("hel").is_empty());
        assert!(tracker.backspace().is_empty());
        let word = tracker.commit("lo ");
        assert_eq!(
            (word.total(false).inputs, word.total(false).corrected),
            (1, 1)
        );
    }
}

#[cfg(test)]
mod cross_platform_policy_tests {
    use super::*;
    #[test]
    fn numeric_bridge_preserves_exact_buckets_and_idempotent_max_merge() {
        let mut input = InputStats::default();
        input.record(true, 27, true, 2);
        input.record(false, 10, false, 0);
        let values: Vec<_> = input.values().collect();
        let mut restored = InputStats::from_values(&values).unwrap_or_default();
        assert_eq!(restored, input);
        restored.merge_max(&input);
        assert_eq!(restored, input);
        assert_eq!(restored.grouped(true)[5].units, 27);
        assert!(InputStats::from_values(&values[..121]).is_none());
    }
    #[test]
    fn mixed_commit_ignores_digits_and_allocates_time_equally() {
        let counts = commit_counts("你a1", 1000);
        assert_eq!(
            (
                counts.chinese,
                counts.english,
                counts.chinese_ms,
                counts.english_ms
            ),
            (1, 1, 500, 500)
        );
        let counts = commit_counts("123 🙂", 1000);
        assert_eq!(
            (counts.total(), counts.chinese_ms, counts.english_ms),
            (0, 0, 0)
        );
        let counts = commit_counts("你a", u64::MAX);
        assert_eq!(counts.chinese_ms + counts.english_ms, u64::MAX);
    }
    #[test]
    fn all_clients_round_speed_halves_up_and_apply_the_same_thresholds() {
        assert_eq!(speed(5, 24000, 5), Some(13));
        assert_eq!(speed(4, 24000, 5), None);
        assert_eq!(speed(5, 9999, 5), None);
    }
}
