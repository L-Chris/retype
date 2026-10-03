//! 异步加载的词库包装器。
//!
//! TIP DLL 被注入到宿主进程，`ActivateEx` 跑在**宿主应用的 UI 线程**上。
//! 在那里同步读一个几百 MB 的词库文件，等于让用户的记事本卡住几秒 ——
//! 这是 ARCHITECTURE.md P1 明确禁止的。
//!
//! 所以词库必须异步加载：激活时先用空词库（降级成原样字母），
//! 后台线程加载完再热替换，用户的键盘从头到尾没有卡过。
//! 对应 §7 降级矩阵里的「系统词库缺失 → 单字模式」，只是这里连单字模式
//! 都要等加载完成，加载前是「原样字母」这一档。

use retype_pinyin::lexicon::{LexEntry, Lexicon};
use retype_types::SyllableId;
use std::path::Path;
use std::sync::{Arc, RwLock};

#[derive(Default)]
pub struct AsyncDict {
    inner: RwLock<Option<Arc<dyn Lexicon>>>,
    total_frequency: RwLock<f64>,
}

// `Arc<dyn Lexicon>` 没有 Debug，手写一个带加载状态的版本
impl std::fmt::Debug for AsyncDict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncDict")
            .field("loaded", &self.is_loaded())
            .field("entries", &self.len())
            .finish()
    }
}

impl AsyncDict {
    pub fn empty() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn with(dict: Arc<dyn Lexicon>) -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(Some(dict)),
            total_frequency: RwLock::new(1.0),
        })
    }

    pub fn is_loaded(&self) -> bool {
        self.read().is_some()
    }

    /// 热替换底层词库。已解码的候选不会回溯更新，下一次按键自然生效。
    pub fn install(&self, dict: Arc<dyn Lexicon>) {
        if let Ok(mut g) = self.inner.write() {
            *g = Some(dict);
        }
    }

    pub fn install_memory(&self, dict: crate::MemoryDict) {
        if let Ok(mut total) = self.total_frequency.write() {
            *total = dict.total_frequency();
        }
        self.install(Arc::new(dict));
    }

    pub fn install_binary(&self, dict: Arc<crate::binary::Dictionary>) {
        if let Ok(mut total) = self.total_frequency.write() {
            *total = dict.total_frequency();
        }
        self.install(dict);
    }

    pub fn total_frequency(&self) -> f64 {
        self.total_frequency.read().map_or(1.0, |value| *value)
    }

    pub fn uninstall(&self) {
        if let Ok(mut g) = self.inner.write() {
            *g = None;
        }
    }

    fn read(&self) -> Option<Arc<dyn Lexicon>> {
        match self.inner.read() {
            Ok(g) => g.clone(),
            // 中毒恢复：加载线程 panic 不该让键盘失灵（P2）
            Err(p) => p.into_inner().clone(),
        }
    }
}

impl Lexicon for AsyncDict {
    fn lookup(&self, syllables: &[SyllableId], out: &mut Vec<LexEntry>) {
        if let Some(d) = self.read() {
            d.lookup(syllables, out);
        }
    }

    fn has_prefix(&self, syllables: &[SyllableId]) -> bool {
        self.read().is_some_and(|d| d.has_prefix(syllables))
    }

    fn lookup_initials(&self, initials: &[u8], out: &mut Vec<LexEntry>) {
        if let Some(d) = self.read() {
            d.lookup_initials(initials, out);
        }
    }

    fn len(&self) -> usize {
        self.read().map(|d| d.len()).unwrap_or(0)
    }
}

/// 词库加载失败时的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FallbackPolicy {
    /// 用全量单字词库兜底：只能打单字，但一定打得出来
    #[default]
    SingleChar,
    /// 保持空词库：原样字母
    Empty,
}

/// 在后台线程加载「已注音词库」（`tools/dict-build` 的产物）。
///
/// 返回的 handle 只用于测试/诊断等待；生产代码不该 join 它，
/// 因为**等待本身就违反了 P1**。
pub fn spawn_loader<P: AsRef<Path>>(
    path: P,
    target: Arc<AsyncDict>,
    fallback: FallbackPolicy,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let path = path.as_ref().to_path_buf();
    std::thread::Builder::new()
        .name("retype-dict-load".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            let loaded = if path.extension().is_some_and(|ext| ext == "bin") {
                crate::binary::open_shared(&path).map_err(|e| e.to_string())
            } else {
                std::fs::File::open(&path)
                    .map_err(|e| format!("打不开词库 {}: {e}", path.display()))
                    .and_then(|f| {
                        let reader = std::io::BufReader::with_capacity(1 << 16, f);
                        crate::memory::load_annotated(reader)
                            .map(|(d, _)| Arc::new(crate::binary::Dictionary::from(d)))
                            .map_err(|e| e.to_string())
                    })
            };
            match loaded {
                Ok(dict) => {
                    tracing::info!(
                        "词库加载完成: {} 词条 / {} 行，耗时 {:?}",
                        dict.len(),
                        dict.len(),
                        started.elapsed()
                    );
                    target.install_binary(dict);
                }
                Err(e) => {
                    tracing::error!("词库加载失败，降级: {e}");
                    match fallback {
                        FallbackPolicy::SingleChar => {
                            target.install_memory(crate::memory::single_char_fallback());
                        }
                        FallbackPolicy::Empty => {}
                    }
                }
            }
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::{annotate, from_pairs, MemoryDict};

    fn ids(py: &str) -> Vec<SyllableId> {
        annotate::parse_pinyin(py).unwrap()
    }

    #[test]
    fn unloaded_dict_returns_nothing_but_does_not_panic() {
        let d = AsyncDict::empty();
        let mut out = Vec::new();
        d.lookup(&ids("ni hao"), &mut out);
        assert!(out.is_empty());
        assert!(!d.has_prefix(&ids("ni")));
        assert_eq!(d.len(), 0);
        assert!(d.is_empty());
    }

    #[test]
    fn hot_swap_takes_effect_immediately() {
        let d = AsyncDict::empty();
        assert!(!d.has_prefix(&ids("ni hao")));
        let (sys, _) = from_pairs([("你好", "ni hao", 5000.0)]);
        d.install(Arc::new(sys));
        assert!(d.has_prefix(&ids("ni hao")));
        let mut out = Vec::new();
        d.lookup(&ids("ni hao"), &mut out);
        assert_eq!(&*out[0].text, "你好");
    }

    #[test]
    fn loader_thread_installs_the_dict() {
        let dir = std::env::temp_dir().join("retype-async-dict-test.tsv");
        std::fs::write(&dir, "你好\tni hao\t5000\n世界\tshi jie\t4000\n").unwrap();
        let d = AsyncDict::empty();
        let h = spawn_loader(&dir, Arc::clone(&d), FallbackPolicy::SingleChar).unwrap();
        h.join().unwrap();
        assert!(d.is_loaded());
        let mut out = Vec::new();
        d.lookup(&ids("ni hao"), &mut out);
        assert_eq!(&*out[0].text, "你好");
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn missing_file_falls_back_to_single_char() {
        let d = AsyncDict::empty();
        let h = spawn_loader(
            std::path::Path::new("Z:/definitely/not/here.tsv"),
            Arc::clone(&d),
            FallbackPolicy::SingleChar,
        )
        .unwrap();
        h.join().unwrap();
        assert!(d.is_loaded(), "加载失败也必须装上兜底词库");
        let mut out = Vec::new();
        d.lookup(&ids("zhong"), &mut out);
        assert!(!out.is_empty(), "降级后仍应能打出单字");
    }

    #[test]
    fn empty_policy_leaves_dict_unloaded() {
        let d = AsyncDict::empty();
        let h = spawn_loader(
            std::path::Path::new("Z:/definitely/not/here.tsv"),
            Arc::clone(&d),
            FallbackPolicy::Empty,
        )
        .unwrap();
        h.join().unwrap();
        assert!(!d.is_loaded());
        assert_eq!(d.len(), 0);
        let _ = MemoryDict::default();
    }
}
