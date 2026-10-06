//! Versioned little-endian dictionary: pre-parsed syllable IDs, no runtime annotation.
use crate::{annotate, DictBuilder, DictError, LoadStats, MemoryDict};
use retype_pinyin::{LexEntry, Lexicon};
use retype_types::SyllableId;
use std::io::{BufRead, Read, Write};
use std::{
    path::Path,
    sync::{Arc, Mutex, OnceLock, Weak},
};
const MAGIC: &[u8; 8] = b"RTDICT01";
const MAX_BYTES: u64 = 128 * 1024 * 1024;

pub enum Dictionary {
    Compact(crate::compact::CompactDict),
    Legacy(MemoryDict),
}
impl Dictionary {
    pub fn total_frequency(&self) -> f64 {
        match self {
            Self::Compact(d) => d.total_frequency(),
            Self::Legacy(d) => d.total_frequency(),
        }
    }
    pub fn node_count(&self) -> usize {
        match self {
            Self::Compact(d) => d.node_count(),
            Self::Legacy(d) => d.node_count(),
        }
    }
    pub fn image_bytes(&self) -> usize {
        match self {
            Self::Compact(d) => d.image_bytes(),
            Self::Legacy(_) => 0,
        }
    }
    #[cfg(feature = "memory-profile")]
    pub fn allocation_stats(&self) -> [usize; 7] {
        match self {
            Self::Legacy(d) => d.allocation_stats(),
            Self::Compact(_) => [0; 7],
        }
    }
}
impl From<MemoryDict> for Dictionary {
    fn from(dict: MemoryDict) -> Self {
        Self::Legacy(dict)
    }
}
impl Lexicon for Dictionary {
    fn lookup(&self, ids: &[SyllableId], out: &mut Vec<LexEntry>) {
        match self {
            Self::Compact(d) => d.lookup(ids, out),
            Self::Legacy(d) => d.lookup(ids, out),
        }
    }
    fn has_prefix(&self, ids: &[SyllableId]) -> bool {
        match self {
            Self::Compact(d) => d.has_prefix(ids),
            Self::Legacy(d) => d.has_prefix(ids),
        }
    }
    fn len(&self) -> usize {
        match self {
            Self::Compact(d) => d.len(),
            Self::Legacy(d) => d.len(),
        }
    }
}
pub fn compile(reader: impl BufRead, mut writer: impl Write) -> Result<usize, DictError> {
    let mut records = Vec::new();
    let count = compile_legacy(reader, &mut records)?;
    let (dict, _) = load_legacy(records.as_slice())?;
    writer.write_all(&crate::compact::encode(&dict)?)?;
    writer.flush()?;
    Ok(count)
}
fn stats(dict: &Dictionary) -> LoadStats {
    LoadStats {
        lines: dict.len(),
        accepted: dict.len(),
        ..Default::default()
    }
}
pub fn load(reader: impl Read) -> Result<(Dictionary, LoadStats), DictError> {
    let mut data = Vec::new();
    reader
        .take(crate::compact::MAX_BYTES + 1)
        .read_to_end(&mut data)?;
    if data.len() as u64 > crate::compact::MAX_BYTES {
        return Err(DictError::Corrupt("oversized dictionary".into()));
    }
    let dict = if data.starts_with(crate::compact::MAGIC) {
        Dictionary::Compact(crate::compact::CompactDict::open(
            crate::compact::Image::Owned(data.into_boxed_slice()),
        )?)
    } else {
        Dictionary::Legacy(load_legacy(data.as_slice())?.0)
    };
    let stats = stats(&dict);
    Ok((dict, stats))
}
type CacheKey = retype_file_map::FileIdentity;
/// Background-thread load: weak ownership shares validated images without
/// keeping obsolete dictionary generations alive after the last session exits.
pub fn open_shared(path: &Path) -> Result<Arc<Dictionary>, DictError> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<CacheKey, Weak<Dictionary>>>> =
        OnceLock::new();
    let image = Arc::new(retype_file_map::ReadOnlyFile::open(
        path,
        crate::compact::MAX_BYTES,
    )?);
    // The opened file already supplies a stable volume/file identity. Windows
    // AppContainers can read/map it while GetFinalPathNameByHandleW (used by
    // canonicalize) is denied. A path query must not invalidate a readable image.
    let key = image.identity();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(dict) = cache.get(&key).and_then(Weak::upgrade) {
        return Ok(dict);
    }
    cache.retain(|_, v| v.strong_count() > 0);
    let dict = if image.as_ref().as_ref().starts_with(crate::compact::MAGIC) {
        Dictionary::Compact(crate::compact::CompactDict::open(
            crate::compact::Image::Mapped(image),
        )?)
    } else {
        Dictionary::Legacy(load_legacy(image.as_ref().as_ref())?.0)
    };
    let dict = Arc::new(dict);
    cache.insert(key, Arc::downgrade(&dict));
    Ok(dict)
}

fn compile_legacy(reader: impl BufRead, mut writer: impl Write) -> Result<usize, DictError> {
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

fn load_legacy(reader: impl Read) -> Result<(MemoryDict, LoadStats), DictError> {
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
    fn compiled_queries_match_legacy_scores_order_and_prefixes() -> Result<(), DictError> {
        let input="你好\tni hao\t5000\n你\tni\t100\n泥\tni\t10\n你好\tni hao\t200\n额度\te du\t50\n我是\two shi\t400\n奇数\tji shu\t300\n快乐\tkuai le\t30\n音乐\tyin yue\t40\n";
        let mut old = Vec::new();
        compile_legacy(input.as_bytes(), &mut old)?;
        let (legacy, _) = load(old.as_slice())?;
        let mut bytes = Vec::new();
        compile(input.as_bytes(), &mut bytes)?;
        let (compact, _) = load(bytes.as_slice())?;
        assert_eq!(legacy.len(), compact.len());
        assert_eq!(legacy.total_frequency(), compact.total_frequency());
        for key in [
            "ni", "ni hao", "e du", "wo shi", "ji shu", "kuai le", "yin yue", "ni ma", "shi", "ma",
        ] {
            let ids =
                annotate::parse_pinyin(key).ok_or_else(|| DictError::Corrupt("fixture".into()))?;
            for prefix in 0..=ids.len() {
                assert_eq!(
                    legacy.has_prefix(&ids[..prefix]),
                    compact.has_prefix(&ids[..prefix])
                );
            }
            let mut a = Vec::new();
            let mut b = Vec::new();
            legacy.lookup(&ids, &mut a);
            compact.lookup(&ids, &mut b);
            assert_eq!(
                a.iter()
                    .map(|e| (&e.text, e.logp.to_bits(), e.flags))
                    .collect::<Vec<_>>(),
                b.iter()
                    .map(|e| (&e.text, e.logp.to_bits(), e.flags))
                    .collect::<Vec<_>>()
            );
        }
        for offset in [8, 12, 16, 20, 24, 32, 40, 64, 68, 72, 76] {
            let mut broken = bytes.clone();
            broken[offset..offset + if offset == 32 { 8 } else { 4 }].fill(255);
            assert!(
                load(broken.as_slice()).is_err(),
                "accepted invalid field at {offset}"
            );
        }
        Ok(())
    }
    #[test]
    fn mapped_cache_shares_and_replacement_keeps_existing_generation_valid() -> Result<(), DictError>
    {
        let root = std::env::temp_dir().join(format!(
            "retype-mapped-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir(&root)?;
        let path = root.join("test.bin");
        let mut bytes = Vec::new();
        compile("你\tni\t100\n".as_bytes(), &mut bytes)?;
        std::fs::write(&path, &bytes)?;
        let a = open_shared(&path)?;
        let b = open_shared(&path)?;
        assert!(Arc::ptr_eq(&a, &b));
        let alias = root.join("alias.bin");
        std::fs::hard_link(&path, &alias)?;
        let aliased = open_shared(&alias)?;
        assert!(Arc::ptr_eq(&a, &aliased));
        drop(aliased);
        std::fs::remove_file(alias)?;
        #[cfg(windows)]
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
        let replacement = root.join("next.bin");
        bytes.clear();
        compile("泥\tni\t100\n".as_bytes(), &mut bytes)?;
        std::fs::write(&replacement, bytes)?;
        // Replacements may have the same size and filesystem timestamp.
        // Snapshot identity must still distinguish their contents.
        #[cfg(not(windows))]
        std::fs::OpenOptions::new()
            .write(true)
            .open(&replacement)?
            .set_times(std::fs::FileTimes::new().set_modified(path.metadata()?.modified()?))?;
        std::fs::rename(replacement, &path)?;
        let c = open_shared(&path)?;
        assert!(!Arc::ptr_eq(&a, &c));
        let ids =
            annotate::parse_pinyin("ni").ok_or_else(|| DictError::Corrupt("fixture".into()))?;
        let mut old = Vec::new();
        let mut new = Vec::new();
        a.lookup(&ids, &mut old);
        c.lookup(&ids, &mut new);
        assert_eq!(&*old[0].text, "你");
        assert_eq!(&*new[0].text, "泥");
        let weak = Arc::downgrade(&a);
        drop(a);
        drop(b);
        assert!(weak.upgrade().is_none());
        drop(c);
        std::fs::remove_file(path)?;
        std::fs::remove_dir(root)?;
        Ok(())
    }
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
