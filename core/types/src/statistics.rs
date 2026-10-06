//! Streaming English word boundaries, shared by the desktop and mobile adapters.
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
