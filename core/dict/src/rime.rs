//! Wanxiang Base readings shared by the desktop builder and mobile downloads.
use crate::annotate;
pub fn plain_pinyin(raw: &str, word_len: usize) -> Option<String> {
    let mut out = String::new();
    let mut count = 0;
    for item in raw.split_whitespace() {
        if count > 0 {
            out.push(' ');
        }
        for c in item.chars() {
            if matches!(c, '\u{0300}'..='\u{036f}') {
                continue;
            }
            out.push(match c {
                'ā' | 'á' | 'ǎ' | 'à' => 'a',
                'ē' | 'é' | 'ě' | 'è' | 'ê' => 'e',
                'ī' | 'í' | 'ǐ' | 'ì' => 'i',
                'ō' | 'ó' | 'ǒ' | 'ò' => 'o',
                'ū' | 'ú' | 'ǔ' | 'ù' => 'u',
                'ü' | 'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' => 'v',
                'ń' | 'ň' | 'ǹ' => 'n',
                'ḿ' => 'm',
                c if c.is_ascii_alphabetic() => c.to_ascii_lowercase(),
                _ => return None,
            });
        }
        count += 1;
    }
    (count == word_len && annotate::parse_pinyin(&out)?.len() == count).then_some(out)
}
pub fn tsv(reader: impl std::io::BufRead) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let mut body = false;
    let mut output = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        let line = line.trim();
        if !body {
            body = line == "...";
            continue;
        }
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let mut cols = line.split('\t');
        let (Some(word), Some(reading), Some(weight)) = (cols.next(), cols.next(), cols.next())
        else {
            continue;
        };
        let Ok(weight) = weight.parse::<f64>() else {
            continue;
        };
        let count = word.chars().count();
        if count > 8 || !annotate::all_han(word) || !weight.is_finite() || weight <= 0.0 {
            continue;
        }
        if let Some(reading) = plain_pinyin(reading, count) {
            writeln!(output, "{word}\t{reading}\t{weight}").map_err(|e| e.to_string())?;
        }
    }
    if output.is_empty() {
        return Err("词库没有有效词条".into());
    }
    Ok(output)
}
