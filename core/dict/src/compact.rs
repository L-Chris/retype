//! Validated contiguous dictionary tables. Offsets are decoded, never cast.
use crate::{DictError, MemoryDict};
use retype_pinyin::{LexEntry, Lexicon};
use retype_types::SyllableId;
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct QueryCache {
    nodes: std::collections::HashMap<usize, Arc<[LexEntry]>>,
    entries: usize,
    bytes: usize,
}

pub const MAGIC: &[u8; 8] = b"RTDICT02";
const HEADER: usize = 64;
pub const MAX_BYTES: u64 = 256 * 1024 * 1024;
pub enum Image {
    Owned(Box<[u8]>),
    Mapped(Arc<retype_file_map::ReadOnlyFile>),
}
impl Image {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Owned(b) => b,
            Self::Mapped(m) => m.as_ref().as_ref(),
        }
    }
}
pub struct CompactDict {
    image: Image,
    entries: usize,
    nodes: usize,
    edges_at: usize,
    entries_at: usize,
    terminals_at: usize,
    text_at: usize,
    total: f64,
    roots: Box<[u32]>,
    pairs: Option<(usize, Box<[u32]>)>,
    cache: Mutex<QueryCache>,
    prefixes: Mutex<std::collections::HashMap<Vec<SyllableId>, Option<usize>>>,
}
fn bad() -> DictError {
    DictError::Corrupt("invalid compact dictionary".into())
}
fn u32_at(data: &[u8], at: usize) -> u32 {
    let Some(b) = data.get(at..at.saturating_add(4)) else {
        return 0;
    };
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
fn u64_at(data: &[u8], at: usize) -> u64 {
    let Some(b) = data.get(at..at.saturating_add(8)) else {
        return 0;
    };
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}
impl CompactDict {
    pub fn open(image: Image) -> Result<Self, DictError> {
        let b = image.bytes();
        if b.len() < HEADER || b.len() as u64 > MAX_BYTES || &b[..8] != MAGIC {
            return Err(bad());
        }
        let entries = u32_at(b, 8) as usize;
        let nodes = u32_at(b, 12) as usize;
        let edges = u32_at(b, 16) as usize;
        let terminals = u32_at(b, 20) as usize;
        let text_len = usize::try_from(u64_at(b, 24)).map_err(|_| bad())?;
        let total = f64::from_bits(u64_at(b, 32));
        if entries == 0
            || nodes == 0
            || edges != nodes - 1
            || terminals != entries
            || !total.is_finite()
            || total < 1.0
            || b[40..HEADER].iter().any(|v| *v != 0)
        {
            return Err(bad());
        }
        let end = |start: usize, count: usize, size: usize| -> Result<usize, DictError> {
            start
                .checked_add(count.checked_mul(size).ok_or_else(bad)?)
                .ok_or_else(bad)
        };
        let edges_at = end(HEADER, nodes, 16)?;
        let entries_at = end(edges_at, edges, 8)?;
        let terminals_at = end(entries_at, entries, 16)?;
        let text_at = end(terminals_at, terminals, 4)?;
        if end(text_at, text_len, 1)? != b.len() {
            return Err(bad());
        }
        let mut dict = Self {
            image,
            entries,
            nodes,
            edges_at,
            entries_at,
            terminals_at,
            text_at,
            total,
            roots: Box::default(),
            pairs: None,
            cache: Mutex::new(QueryCache::default()),
            prefixes: Mutex::new(std::collections::HashMap::new()),
        };
        dict.validate()?;
        let (start, count, _, _) = dict.node(0);
        let b = dict.image.bytes();
        if count > 0 {
            let maximum = u32_at(b, dict.edges_at + (start + count - 1) * 8) as usize;
            let mut roots = vec![0; maximum + 1];
            for edge in start..start + count {
                let at = dict.edges_at + edge * 8;
                roots[u32_at(b, at) as usize] = u32_at(b, at + 4);
            }
            dict.roots = roots.into_boxed_slice();
        }
        // A bounded dense first-two-syllable index avoids locks and repeated
        // binary searches on the most common decoder queries. At most 1 MiB.
        let mut width = dict.roots.len();
        for &root in dict.roots.iter().filter(|n| **n != 0) {
            let (start, count, _, _) = dict.node(root as usize);
            if count > 0 {
                width = width.max(u32_at(b, dict.edges_at + (start + count - 1) * 8) as usize + 1);
            }
        }
        if width <= 512 {
            let mut pairs = vec![0; width * width];
            for (first, &root) in dict.roots.iter().enumerate().filter(|(_, n)| **n != 0) {
                let (start, count, _, _) = dict.node(root as usize);
                for edge in start..start + count {
                    let at = dict.edges_at + edge * 8;
                    pairs[first * width + u32_at(b, at) as usize] = u32_at(b, at + 4);
                }
            }
            dict.pairs = Some((width, pairs.into_boxed_slice()));
        }
        Ok(dict)
    }
    fn validate(&self) -> Result<(), DictError> {
        let b = self.image.bytes();
        let mut parents = vec![false; self.nodes];
        let mut seen = vec![false; self.entries];
        let mut edges = 0;
        let mut terminals = 0;
        for node in 0..self.nodes {
            let (es, ec, ts, tc) = self.node(node);
            if (node == 0 && tc != 0) || (node != 0 && ec == 0 && tc == 0) {
                return Err(bad());
            }
            if es != edges
                || ts != terminals
                || ec > self.nodes - 1 - edges
                || tc > self.entries - terminals
            {
                return Err(bad());
            }
            let mut previous = None;
            for edge in es..es + ec {
                let at = self.edges_at + edge * 8;
                let label = u32_at(b, at);
                let child = u32_at(b, at + 4) as usize;
                if label > u16::MAX as u32
                    || retype_pinyin::syllables::name_of(label as u16).is_none()
                    || previous.is_some_and(|p| p >= label)
                    || child <= node
                    || child >= self.nodes
                    || parents[child]
                {
                    return Err(bad());
                }
                parents[child] = true;
                previous = Some(label);
            }
            for terminal in ts..ts + tc {
                let entry = u32_at(b, self.terminals_at + terminal * 4) as usize;
                if entry >= self.entries || seen[entry] {
                    return Err(bad());
                }
                seen[entry] = true;
            }
            edges += ec;
            terminals += tc;
        }
        if edges != self.nodes - 1
            || terminals != self.entries
            || parents[1..].iter().any(|v| !*v)
            || seen.iter().any(|v| !*v)
        {
            return Err(bad());
        }
        for entry in 0..self.entries {
            let at = self.entries_at + entry * 16;
            let start = u32_at(b, at) as usize;
            let length = u32_at(b, at + 4) as usize;
            let logp = f32::from_bits(u32_at(b, at + 8));
            if length == 0
                || length > 65535
                || !logp.is_finite()
                || logp > 0.0
                || b[at + 13..at + 16].iter().any(|v| *v != 0)
            {
                return Err(bad());
            }
            let end = start.checked_add(length).ok_or_else(bad)?;
            let text = b
                .get(self.text_at..)
                .and_then(|text| text.get(start..end))
                .ok_or_else(bad)?;
            std::str::from_utf8(text).map_err(|_| bad())?;
        }
        Ok(())
    }
    fn node(&self, id: usize) -> (usize, usize, usize, usize) {
        let b = self.image.bytes();
        let at = HEADER + id * 16;
        (
            u32_at(b, at) as usize,
            u32_at(b, at + 4) as usize,
            u32_at(b, at + 8) as usize,
            u32_at(b, at + 12) as usize,
        )
    }
    fn find(&self, ids: &[SyllableId]) -> Option<usize> {
        if ids.is_empty() {
            return Some(0);
        }
        if ids.len() == 1 {
            return self
                .roots
                .get(ids[0] as usize)
                .copied()
                .filter(|node| *node != 0)
                .map(|node| node as usize);
        }
        if ids.len() == 2 {
            if let Some((width, pairs)) = &self.pairs {
                return if (ids[0] as usize) < *width && (ids[1] as usize) < *width {
                    let node = pairs[ids[0] as usize * width + ids[1] as usize];
                    (node != 0).then_some(node as usize)
                } else {
                    None
                };
            }
        }
        if ids.len() <= 32 {
            let cached = self
                .prefixes
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(ids)
                .copied();
            if let Some(cached) = cached {
                return cached;
            }
        }
        let found = self.find_uncached(ids);
        if ids.len() <= 32 {
            let mut cache = self.prefixes.lock().unwrap_or_else(|p| p.into_inner());
            if cache.len() >= 8192 {
                cache.clear();
            }
            cache.insert(ids.to_vec(), found);
        }
        found
    }
    fn find_uncached(&self, ids: &[SyllableId]) -> Option<usize> {
        let b = self.image.bytes();
        let mut node = self
            .roots
            .get(*ids.first()? as usize)
            .copied()
            .filter(|node| *node != 0)? as usize;
        for &id in &ids[1..] {
            let (start, count, _, _) = self.node(node);
            let mut low = 0;
            let mut high = count;
            while low < high {
                let middle = (low + high) / 2;
                if u32_at(b, self.edges_at + (start + middle) * 8) < id as u32 {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
            if low == count || u32_at(b, self.edges_at + (start + low) * 8) != id as u32 {
                return None;
            }
            node = u32_at(b, self.edges_at + (start + low) * 8 + 4) as usize;
        }
        Some(node)
    }
    pub fn total_frequency(&self) -> f64 {
        self.total
    }
    pub fn node_count(&self) -> usize {
        self.nodes
    }
    pub fn image_bytes(&self) -> usize {
        self.image.bytes().len()
    }
}
impl Lexicon for CompactDict {
    fn lookup(&self, ids: &[SyllableId], out: &mut Vec<LexEntry>) {
        let Some(node) = self.find(ids) else {
            return;
        };
        let cached = self
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .nodes
            .get(&node)
            .cloned();
        if let Some(cached) = cached {
            out.extend(cached.iter().cloned());
            return;
        }
        let (_, _, start, count) = self.node(node);
        if count == 0 {
            return;
        }
        let b = self.image.bytes();
        let mut hits = Vec::with_capacity(count);
        for terminal in start..start + count {
            let entry = u32_at(b, self.terminals_at + terminal * 4) as usize;
            let at = self.entries_at + entry * 16;
            let start = self.text_at + u32_at(b, at) as usize;
            let end = start + u32_at(b, at + 4) as usize;
            if let Ok(text) = std::str::from_utf8(&b[start..end]) {
                hits.push(LexEntry {
                    text: Arc::from(text),
                    logp: f32::from_bits(u32_at(b, at + 8)),
                    flags: b[at + 12],
                });
            }
        }
        // Only queried nodes are materialized. Bound both strings and entry
        // count so uncommon keys cannot grow this per-image cache indefinitely.
        let bytes: usize = hits
            .iter()
            .map(|e| {
                std::mem::size_of::<LexEntry>() + 2 * std::mem::size_of::<usize>() + e.text.len()
            })
            .sum();
        if count <= 16_384 && bytes <= 2 * 1024 * 1024 {
            let hits: Arc<[LexEntry]> = hits.into();
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            if !cache.nodes.contains_key(&node) {
                if cache.entries + count > 16_384
                    || cache.bytes + bytes > 2 * 1024 * 1024
                    || cache.nodes.len() >= 512
                {
                    cache.nodes.clear();
                    cache.entries = 0;
                    cache.bytes = 0;
                }
                cache.entries += count;
                cache.bytes += bytes;
                cache.nodes.insert(node, Arc::clone(&hits));
            }
            drop(cache);
            out.extend(hits.iter().cloned());
        } else {
            out.extend(hits);
        }
    }
    fn has_prefix(&self, ids: &[SyllableId]) -> bool {
        self.find(ids).is_some()
    }
    fn len(&self) -> usize {
        self.entries
    }
}
pub fn encode(dict: &MemoryDict) -> Result<Vec<u8>, DictError> {
    let nodes = &dict.trie.nodes;
    let edges: usize = nodes.iter().map(|n| n.children.len()).sum();
    let terminals: usize = nodes.iter().map(|n| n.entries.len()).sum();
    let number = |n: usize| u32::try_from(n).map_err(|_| bad());
    let mut text = Vec::new();
    let mut strings = std::collections::HashMap::<&str, u32>::new();
    let mut records = Vec::with_capacity(dict.entries.len() * 16);
    for entry in &dict.entries {
        let position = if let Some(position) = strings.get(entry.text.as_ref()) {
            *position
        } else {
            let position = number(text.len())?;
            text.extend_from_slice(entry.text.as_bytes());
            strings.insert(&entry.text, position);
            position
        };
        records.extend_from_slice(&position.to_le_bytes());
        records.extend_from_slice(&number(entry.text.len())?.to_le_bytes());
        records.extend_from_slice(&entry.logp.to_le_bytes());
        records.push(entry.flags);
        records.extend_from_slice(&[0; 3]);
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    for count in [dict.entries.len(), nodes.len(), edges, terminals] {
        bytes.extend_from_slice(&number(count)?.to_le_bytes());
    }
    bytes.extend_from_slice(&(text.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&dict.total_frequency().to_le_bytes());
    bytes.resize(HEADER, 0);
    let mut edge = 0;
    let mut terminal = 0;
    for node in nodes {
        for value in [edge, node.children.len(), terminal, node.entries.len()] {
            bytes.extend_from_slice(&number(value)?.to_le_bytes());
        }
        edge += node.children.len();
        terminal += node.entries.len();
    }
    for node in nodes {
        for (label, child) in &node.children {
            bytes.extend_from_slice(&(*label as u32).to_le_bytes());
            bytes.extend_from_slice(&child.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&records);
    for node in nodes {
        for id in &node.entries {
            bytes.extend_from_slice(&id.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&text);
    if bytes.len() as u64 > MAX_BYTES {
        return Err(bad());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_cache_is_bounded_and_eviction_preserves_results() -> Result<(), DictError> {
        let ids = crate::annotate::parse_pinyin("ni hao ma wo shi yin yue e du qi shu zhi")
            .ok_or_else(bad)?;
        let mut builder = MemoryDict::builder();
        let mut keys = Vec::new();
        for i in 0..1000 {
            let key = vec![ids[i % 12], ids[i / 12 % 12], ids[i / 144 % 12]];
            builder.push(&format!("词{i}"), key.clone(), 1.0, 0);
            keys.push(key);
        }
        let dict = CompactDict::open(Image::Owned(encode(&builder.build())?.into_boxed_slice()))?;
        for (i, key) in keys.iter().enumerate() {
            let mut hits = Vec::new();
            dict.lookup(key, &mut hits);
            assert_eq!(&*hits[0].text, format!("词{i}"));
            let cache = dict.cache.lock().unwrap_or_else(|p| p.into_inner());
            assert!(cache.nodes.len() <= 512);
            assert!(cache.entries <= 16384);
            assert!(cache.bytes <= 2 * 1024 * 1024);
        }
        let mut again = Vec::new();
        dict.lookup(&keys[0], &mut again);
        assert_eq!(&*again[0].text, "词0");
        for i in 0..10_000 {
            assert!(!dict.has_prefix(&[ids[0], (1000 + i) as u16]));
            assert!(
                dict.prefixes
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .len()
                    <= 8192
            );
        }
        Ok(())
    }
}
