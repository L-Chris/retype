//! Versioned little-endian dictionary: pre-parsed syllable IDs, no runtime annotation.
use crate::{annotate, DictBuilder, DictError, LoadStats, MemoryDict};
use std::io::{BufRead, Read, Write};
const MAGIC: &[u8; 8] = b"RTDICT01";
const MAX_BYTES: u64 = 128 * 1024 * 1024;

pub fn compile(reader: impl BufRead, mut writer: impl Write) -> Result<usize, DictError> {
    let mut records = Vec::new();
    let mut count = 0u32;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<_> = line.split('\t').collect();
        if cols.len() < 3 {
            return Err(DictError::Corrupt("missing TSV fields".into()));
        }
        let ids = annotate::parse_pinyin(cols[1])
            .ok_or_else(|| DictError::UnknownSyllable(cols[1].into()))?;
        let frequency: f64 = cols[2]
            .parse()
            .map_err(|_| DictError::Corrupt("invalid frequency".into()))?;
        if !frequency.is_finite()
            || frequency <= 0.0
            || cols[0].is_empty()
            || cols[0].len() > 65535
            || ids.len() > 255
        {
            return Err(DictError::Corrupt("invalid entry".into()));
        }
        records.extend_from_slice(&(cols[0].len() as u16).to_le_bytes());
        records.push(ids.len() as u8);
        records.extend_from_slice(&frequency.to_le_bytes());
        records.extend_from_slice(cols[0].as_bytes());
        for id in ids {
            records.extend_from_slice(&id.to_le_bytes());
        }
        count += 1;
    }
    if count == 0 || records.len() as u64 > MAX_BYTES - 12 {
        return Err(DictError::Corrupt("empty or oversized dictionary".into()));
    }
    writer.write_all(MAGIC)?;
    writer.write_all(&count.to_le_bytes())?;
    writer.write_all(&records)?;
    writer.flush()?;
    Ok(count as usize)
}

pub fn load(reader: impl Read) -> Result<(MemoryDict, LoadStats), DictError> {
    let mut data = Vec::new();
    reader.take(MAX_BYTES + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_BYTES {
        return Err(DictError::Corrupt("oversized dictionary".into()));
    }
    let mut remaining = data.as_slice();
    fn take<'a>(data: &mut &'a [u8], n: usize) -> Result<&'a [u8], DictError> {
        if n > data.len() {
            return Err(DictError::Corrupt("truncated dictionary".into()));
        }
        let (value, tail) = data.split_at(n);
        *data = tail;
        Ok(value)
    }
    if take(&mut remaining, 8)? != MAGIC {
        return Err(DictError::Corrupt("unsupported dictionary version".into()));
    }
    let bytes = take(&mut remaining, 4)?;
    let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if count == 0 || count > remaining.len() / 14 {
        return Err(DictError::Corrupt("invalid entry count".into()));
    }
    let mut builder = DictBuilder::default();
    builder.reserve(count);
    for _ in 0..count {
        let header = take(&mut remaining, 11)?;
        let length = u16::from_le_bytes([header[0], header[1]]) as usize;
        let syllables = header[2] as usize;
        let frequency = f64::from_le_bytes([
            header[3], header[4], header[5], header[6], header[7], header[8], header[9], header[10],
        ]);
        let word = std::str::from_utf8(take(&mut remaining, length)?)
            .map_err(|_| DictError::Corrupt("invalid UTF-8".into()))?;
        if word.is_empty() || syllables == 0 || !frequency.is_finite() || frequency <= 0.0 {
            return Err(DictError::Corrupt("invalid entry".into()));
        }
        let mut ids = Vec::with_capacity(syllables);
        for _ in 0..syllables {
            let b = take(&mut remaining, 2)?;
            let id = u16::from_le_bytes([b[0], b[1]]);
            if retype_pinyin::syllables::name_of(id).is_none() {
                return Err(DictError::Corrupt("unknown syllable".into()));
            }
            ids.push(id);
        }
        builder.push(word, ids, frequency, 0);
    }
    if !remaining.is_empty() {
        return Err(DictError::Corrupt("unexpected trailing data".into()));
    }
    Ok((
        builder.build(),
        LoadStats {
            lines: count,
            accepted: count,
            ..Default::default()
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use retype_pinyin::Lexicon;
    #[test]
    fn roundtrip_and_every_truncation() -> Result<(), DictError> {
        let mut bytes = Vec::new();
        assert_eq!(
            compile("你好\tni hao\t5000\n你\tni\t100\n".as_bytes(), &mut bytes)?,
            2
        );
        let (dict, _) = load(bytes.as_slice())?;
        let mut out = Vec::new();
        dict.lookup(
            &annotate::parse_pinyin("ni hao").ok_or_else(|| DictError::Corrupt("test".into()))?,
            &mut out,
        );
        assert_eq!(&*out[0].text, "你好");
        for n in 0..bytes.len() {
            assert!(load(&bytes[..n]).is_err(), "accepted truncation at {n}");
        }
        bytes.push(0);
        assert!(load(bytes.as_slice()).is_err());
        Ok(())
    }
}
