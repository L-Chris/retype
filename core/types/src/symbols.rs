//! Shared, bounded symbol rules. Editor adapters own the generated closing ranges.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    pub opening: Option<(char, char)>,
    pub closing: Option<char>,
}

pub fn rules(key: char, chinese: bool) -> Rules {
    let opening = match key {
        '(' => Some(if chinese { ('（', '）') } else { ('(', ')') }),
        '[' => Some(if chinese { ('【', '】') } else { ('[', ']') }),
        '{' => Some(('{', '}')),
        '<' if chinese => Some(('《', '》')),
        '"' => Some(if chinese { ('“', '”') } else { ('"', '"') }),
        '\'' => Some(if chinese {
            ('‘', '’')
        } else {
            ('\'', '\'')
        }),
        '（' => Some(('（', '）')),
        '【' => Some(('【', '】')),
        '《' => Some(('《', '》')),
        '“' => Some(('“', '”')),
        '‘' => Some(('‘', '’')),
        '「' => Some(('「', '」')),
        '『' => Some(('『', '』')),
        _ => None,
    };
    let closing = match key {
        ')' => Some(if chinese { '）' } else { ')' }),
        ']' => Some(if chinese { '】' } else { ']' }),
        '}' => Some('}'),
        '>' if chinese => Some('》'),
        '）' | '】' | '》' | '”' | '’' | '」' | '』' => Some(key),
        '"' | '\'' => opening.map(|(_, close)| close),
        _ => None,
    };
    Rules { opening, closing }
}

pub fn eligible(open: char, close: char, before: Option<char>, after: Option<char>) -> bool {
    // Apostrophes inside words, and an application's existing closer, stay literal.
    after != Some(close) && !(open == '\'' && before.is_some_and(char::is_alphanumeric))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    #[test]
    fn mode_and_quote_rules_keep_word_connectors_and_existing_closers() {
        assert_eq!(rules('(', true).opening, Some(('（', '）')));
        assert_eq!(rules('(', false).opening, Some(('(', ')')));
        assert_eq!(rules('"', true).closing, Some('”'));
        assert_eq!(rules('」', true).closing, Some('」'));
        assert_eq!(rules('<', false).opening, None);
        assert!(!eligible('\'', '\'', Some('n'), None));
        assert!(!eligible('(', ')', None, Some(')')));
        assert!(eligible('（', '）', None, None));
    }
    #[test]
    fn all_openers_have_matching_closer_rules() {
        for chinese in [false, true] {
            for key in "([\"'{（【《“‘「『".chars().chain(chinese.then_some('<')) {
                let (left, right) = rules(key, chinese).opening.expect("supported opener");
                assert_eq!(rules(right, chinese).closing, Some(right));
                assert!(eligible(left, right, None, None));
                assert!(!eligible(left, right, None, Some(right)));
            }
        }
        for key in "a1,.!? ".chars() {
            assert_eq!(
                rules(key, true),
                Rules {
                    opening: None,
                    closing: None
                }
            );
        }
    }
}
