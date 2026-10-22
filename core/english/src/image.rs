use retype_file_map::ReadOnlyFile;
use std::{
    collections::BTreeMap,
    io::{BufRead, Write},
    path::Path,
    sync::{Arc, OnceLock, RwLock},
};

pub const MAGIC: &[u8; 8] = b"RTENGL01";
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
enum Image {
    Mapped(ReadOnlyFile),
    Owned(Box<[u8]>),
}
impl AsRef<[u8]> for Image {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Mapped(m) => m.as_ref(),
            Self::Owned(b) => b,
        }
    }
}
pub struct Vocabulary {
    image: Image,
    count: usize,
    size: usize,
    arena: usize,
}
impl std::fmt::Debug for Vocabulary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vocabulary")
            .field("words", &self.count)
            .finish()
    }
}
impl Vocabulary {
    pub fn open(path: &Path) -> Result<Arc<Self>, String> {
        let image = ReadOnlyFile::open(path, MAX_BYTES).map_err(|e| e.to_string())?;
        // Reuse the already validated mapping across editor sessions.
        static CACHE: OnceLock<
            std::sync::Mutex<
                std::collections::HashMap<
                    retype_file_map::FileIdentity,
                    std::sync::Weak<Vocabulary>,
                >,
            >,
        > = OnceLock::new();
        let identity = image.identity();
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(existing) = cache.get(&identity).and_then(std::sync::Weak::upgrade) {
            return Ok(existing);
        }
        let result = Arc::new(Self::validate(Image::Mapped(image))?);
        cache.retain(|_, weak| weak.strong_count() > 0);
        cache.insert(identity, Arc::downgrade(&result));
        Ok(result)
    }
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, String> {
        Self::validate(Image::Owned(bytes.into_boxed_slice()))
    }
    fn validate(image: Image) -> Result<Self, String> {
        let bytes = image.as_ref();
        let bad = || "Invalid English dictionary image".to_owned();
        if bytes.len() < 24 || bytes.len() as u64 > MAX_BYTES || !bytes.starts_with(MAGIC) {
            return Err(bad());
        }
        let read =
            |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or_default()) as usize;
        let count = read(8);
        let size = read(12);
        if count > 1_000_000 || !size.is_power_of_two() || size < count || size > 1_048_576 {
            return Err(bad());
        }
        let arena = 16 + count * 16 + size * 8;
        if arena > bytes.len() {
            return Err(bad());
        }
        let result = Self {
            image,
            count,
            size,
            arena,
        };
        let mut previous: &[u8] = &[];
        let mut end = 0;
        for id in 0..count {
            let at = 16 + id * 16;
            let start = result.u32_at(at);
            let len = result.u32_at(at + 4);
            if start != end
                || !(1..=64).contains(&len)
                || start
                    .checked_add(len)
                    .is_none_or(|end| end > result.image.as_ref().len() - arena)
            {
                return Err(bad());
            }
            end = start + len;
            let word = &result.image.as_ref()[arena + start..arena + end];
            if word <= previous
                || !word.iter().all(|b| b.is_ascii_lowercase() || *b == b'\'')
                || result.frequency(id) == 0
            {
                return Err(bad());
            }
            previous = word;
        }
        if arena + end != result.image.as_ref().len() {
            return Err(bad());
        }
        let tree = |node| result.u32_at(16 + count * 16 + node * 4);
        for leaf in 0..size {
            if tree(size + leaf)
                != if leaf < count {
                    leaf
                } else {
                    u32::MAX as usize
                }
            {
                return Err(bad());
            }
        }
        for node in (1..size).rev() {
            let a = tree(node * 2);
            let b = tree(node * 2 + 1);
            let best = if a == u32::MAX as usize {
                b
            } else if b == u32::MAX as usize || result.frequency(a) >= result.frequency(b) {
                a
            } else {
                b
            };
            if tree(node) != best {
                return Err(bad());
            }
        }
        Ok(result)
    }
    pub fn empty() -> Self {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&[255; 8]);
        Self {
            image: Image::Owned(bytes.into_boxed_slice()),
            count: 0,
            size: 1,
            arena: 24,
        }
    }
    pub(super) fn u32_at(&self, at: usize) -> usize {
        u32::from_le_bytes(
            self.image.as_ref()[at..at + 4]
                .try_into()
                .unwrap_or_default(),
        ) as usize
    }
    pub fn count(&self) -> usize {
        self.count
    }
    pub(super) fn size(&self) -> usize {
        self.size
    }
    pub(super) fn word(&self, id: usize) -> &str {
        let at = 16 + id * 16;
        std::str::from_utf8(
            &self.image.as_ref()
                [self.arena + self.u32_at(at)..self.arena + self.u32_at(at) + self.u32_at(at + 4)],
        )
        .unwrap_or("")
    }
    pub(super) fn frequency(&self, id: usize) -> u64 {
        let at = 24 + id * 16;
        u64::from_le_bytes(
            self.image.as_ref()[at..at + 8]
                .try_into()
                .unwrap_or_default(),
        )
    }
}

/// Build a deterministic image on the build machine, never on the input thread.
pub fn compile(source: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
    output.write_all(&encode(source)?)
}
fn encode(source: impl BufRead) -> std::io::Result<Vec<u8>> {
    let mut words = BTreeMap::<String, u64>::new();
    for line in source.lines() {
        let line = line?;
        if let Some((word, count)) = line.trim_start_matches('\u{feff}').rsplit_once(' ') {
            if !word.is_empty()
                && word.len() <= 64
                && word.bytes().all(|b| b.is_ascii_lowercase() || b == b'\'')
            {
                if let Ok(count) = count.parse::<u64>() {
                    if count > 0 {
                        let entry = words.entry(word.to_owned()).or_default();
                        *entry = entry.saturating_add(count);
                    }
                }
            }
        }
    }
    let words: Vec<_> = words.into_iter().collect();
    let size = words.len().max(1).next_power_of_two();
    let mut best = vec![u32::MAX; size * 2];
    for i in 0..words.len() {
        best[size + i] = i as u32;
    }
    for i in (1..size).rev() {
        let a = best[i * 2];
        let b = best[i * 2 + 1];
        best[i] = if a == u32::MAX {
            b
        } else if b == u32::MAX || words[a as usize].1 >= words[b as usize].1 {
            a
        } else {
            b
        };
    }
    let mut arena = Vec::new();
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&(words.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(size as u32).to_le_bytes());
    for (word, count) in words {
        bytes.extend_from_slice(&(arena.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(word.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        arena.extend_from_slice(word.as_bytes());
    }
    for id in best {
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    bytes.extend_from_slice(&arena);
    Ok(bytes)
}

static ACTIVE: OnceLock<RwLock<Option<Arc<Vocabulary>>>> = OnceLock::new();
/// Called only by platform resource workers. Existing kernels keep their snapshot.
pub fn install(path: &Path) -> Result<(), String> {
    let image = Vocabulary::open(path)?;
    *ACTIVE
        .get_or_init(Default::default)
        .write()
        .unwrap_or_else(|p| p.into_inner()) = Some(image);
    Ok(())
}
pub fn current() -> Arc<Vocabulary> {
    if let Some(image) = ACTIVE
        .get_or_init(Default::default)
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
    {
        return Arc::clone(image);
    }
    static EMPTY: OnceLock<Arc<Vocabulary>> = OnceLock::new();
    #[cfg(any(test, feature = "test-data"))]
    return Arc::clone(EMPTY.get_or_init(|| {
        let file = std::fs::File::open(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../data/dict/raw/english/frequency.txt"
        ));
        let data = file
            .ok()
            .and_then(|f| encode(std::io::BufReader::new(f)).ok());
        Arc::new(
            data.and_then(|b| Vocabulary::from_bytes(b).ok())
                .unwrap_or_else(Vocabulary::empty),
        )
    }));
    #[cfg(not(any(test, feature = "test-data")))]
    Arc::clone(EMPTY.get_or_init(|| Arc::new(Vocabulary::empty())))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn corrupt_offsets_counts_and_tree_are_rejected_without_panics() {
        let mut bytes = Vec::new();
        compile(&b"hello 10\nhelp 8\nworld 6\n"[..], &mut bytes).unwrap();
        assert_eq!(Vocabulary::from_bytes(bytes.clone()).unwrap().count(), 3);
        for length in 0..bytes.len() {
            assert!(Vocabulary::from_bytes(bytes[..length].to_vec()).is_err());
        }
        for at in [
            8, 12, 16, 20, 32, 36, 48, 52, 68, 72, 76, 80, 84, 88, 96, 100,
        ] {
            let mut corrupt = bytes.clone();
            corrupt[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(Vocabulary::from_bytes(corrupt).is_err(), "offset {at}");
        }
    }
}
