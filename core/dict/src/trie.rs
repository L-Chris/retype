//! 音节序列前缀树。
//!
//! 词格扩展时对每个前缀都要问一次「词库里有没有以这串音节开头的词」，
//! 这是首刷延迟的热点。用 trie 而不是 `HashMap<Vec<u16>, _>`：
//! 35 万词会产生上百万个前缀 key，HashMap 的内存和哈希开销都不可接受。

use retype_types::SyllableId;

#[derive(Debug, Default, Clone)]
pub(crate) struct Node {
    /// 子边，按音节 id 升序，二分查找
    pub(crate) children: Vec<(SyllableId, u32)>,
    /// 恰好在这个前缀上结束的词条下标
    pub(crate) entries: Vec<u32>,
}

#[derive(Debug, Default, Clone)]
pub struct Trie {
    pub(crate) nodes: Vec<Node>,
}

impl Trie {
    pub fn new() -> Self {
        Self {
            nodes: vec![Node::default()],
        }
    }

    /// 预留容量，批量构建时避免反复扩容。
    ///
    /// 注意不能用 `Self { nodes: Vec::with_capacity(..), ..Self::new() }`：
    /// 结构体更新语法会用空的 `nodes` 覆盖掉 `new()` 里刚建好的根节点。
    pub fn with_capacity(capacity: usize) -> Self {
        let mut nodes = Vec::with_capacity(capacity.saturating_add(1).max(2));
        nodes.push(Node::default());
        Self { nodes }
    }

    pub fn insert(&mut self, key: &[SyllableId], entry: u32) {
        let mut cur: u32 = 0;
        for &id in key {
            let existing = {
                let Some(node) = self.nodes.get(cur as usize) else {
                    return;
                };
                node.children
                    .binary_search_by_key(&id, |c| c.0)
                    .ok()
                    .map(|i| node.children[i].1)
            };
            cur = match existing {
                Some(n) => n,
                None => {
                    let n = self.nodes.len() as u32;
                    self.nodes.push(Node::default());
                    let Some(node) = self.nodes.get_mut(cur as usize) else {
                        return;
                    };
                    let pos = node.children.partition_point(|c| c.0 < id);
                    node.children.insert(pos, (id, n));
                    n
                }
            };
        }
        if let Some(node) = self.nodes.get_mut(cur as usize) {
            node.entries.push(entry);
        }
    }

    pub(crate) fn find(&self, key: &[SyllableId]) -> Option<&Node> {
        let mut cur = self.nodes.first()?;
        for &id in key {
            let idx = cur.children.binary_search_by_key(&id, |c| c.0).ok()?;
            let next = cur.children.get(idx)?.1 as usize;
            cur = self.nodes.get(next)?;
        }
        Some(cur)
    }

    #[inline]
    pub fn contains_prefix(&self, key: &[SyllableId]) -> bool {
        self.find(key).is_some()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Allocation sizes for the isolated profiler; absent from normal builds.
    #[cfg(feature = "memory-profile")]
    pub fn allocation_stats(&self) -> [usize; 5] {
        [
            self.nodes.capacity(),
            self.nodes.capacity() * std::mem::size_of::<Node>(),
            self.nodes.len() * std::mem::size_of::<Node>(),
            self.nodes
                .iter()
                .map(|n| n.children.capacity() * std::mem::size_of::<(SyllableId, u32)>())
                .sum(),
            self.nodes
                .iter()
                .map(|n| n.entries.capacity() * std::mem::size_of::<u32>())
                .sum(),
        ]
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn insert_and_find() {
        let mut t = Trie::new();
        t.insert(&[1, 2], 10);
        t.insert(&[1, 2, 3], 11);
        t.insert(&[1, 5], 12);
        assert_eq!(t.find(&[1, 2]).unwrap().entries, vec![10]);
        assert_eq!(t.find(&[1, 2, 3]).unwrap().entries, vec![11]);
        assert!(t.contains_prefix(&[1]));
        assert!(t.contains_prefix(&[1, 2]));
        assert!(!t.contains_prefix(&[1, 4]));
        assert!(!t.contains_prefix(&[9]));
    }

    #[test]
    fn multiple_entries_on_same_key() {
        let mut t = Trie::new();
        t.insert(&[7], 1);
        t.insert(&[7], 2);
        assert_eq!(t.find(&[7]).unwrap().entries, vec![1, 2]);
    }

    #[test]
    fn empty_key_maps_to_root() {
        let t = Trie::new();
        assert!(t.find(&[]).is_some());
        assert!(t.find(&[]).unwrap().entries.is_empty());
    }

    /// 回归：`with_capacity` 曾经用结构体更新语法把根节点覆盖掉，
    /// 导致整棵树为空、所有查询都落空。
    #[test]
    fn with_capacity_keeps_root_node() {
        let mut t = Trie::with_capacity(1024);
        assert_eq!(t.node_count(), 1);
        t.insert(&[1, 2], 7);
        assert_eq!(t.find(&[1, 2]).unwrap().entries, vec![7]);
    }
}
