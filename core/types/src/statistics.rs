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
        let mut words = 0u32;
        for ch in text.chars() {
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
                words = words.saturating_add(self.finish());
            }
        }
        words
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
pub const LENGTH_LABELS: [&str; 8] = ["1", "2", "3", "4", "5–8", "9–16", "17–32", "33+"];
pub fn length_index(n: u64) -> usize {
    match n {
        0..=1 => 0,
        2 => 1,
        3 => 2,
        4 => 3,
        5..=8 => 4,
        9..=16 => 5,
        17..=32 => 6,
        _ => 7,
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct InputStats {
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "empty_lengths"))]
    pub chinese: [LengthCounts; 8],
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "empty_lengths"))]
    pub english: [LengthCounts; 8],
    pub cancelled: u64,
    pub spelling_deletions: u64,
}
#[cfg(feature = "serde")]
fn empty_lengths(rows: &[LengthCounts; 8]) -> bool {
    rows.iter().all(|v| *v == LengthCounts::default())
}
impl InputStats {
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
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "InputStats::is_empty"))]
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
