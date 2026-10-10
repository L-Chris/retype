//! Mock 实现（ADR-0002）。
//!
//! Mock 的价值不在于「假装能用」，而在于**能主动制造故障**：
//! 延迟、失败、hang 死、过期代次 —— 降级路径必须从第一天就可测（ARCHITECTURE.md §7）。
//!
//! `MockLlmReranker` 的重排逻辑是一个刻意简化的「假 AI」：
//! 上下文里出现过的词/二元组得分更高。它没有智能，但足以在 `retype-diag` 里
//! 演示「首刷 → 二刷 → 顺序变化」的完整链路，并让合并规则（P3）可被断言。

use crate::{
    AsrConfig, AsrSession, CloudError, CloudPinyin, LlmReranker, PinyinRequest, PinyinSuggestion,
    RerankRequest, RerankResponse, StreamingAsr,
};
use futures::future::BoxFuture;
use retype_types::{AsrEvent, Candidate, CandidateSource, ContextSnapshot};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Mock 行为配置。所有字段都有「正常」默认值。
#[derive(Debug, Clone)]
pub struct MockConfig {
    /// 人为延迟。用来验证 `CloudClient` 的超时路径。
    pub latency: Duration,
    /// 恒定失败
    pub fail: bool,
    /// 每 N 次调用失败一次（N>=2），确定性地模拟不稳定网络
    pub fail_every: Option<u32>,
    /// 永不返回。验证「超时后放弃等待」而不是「一直挂着」
    pub hang: bool,
    /// 云端热词，会被塞进 `RerankResponse::extra`
    pub hotword: Option<String>,
    /// ASR 脚本：按顺序吐出的三段式事件
    pub script: Vec<AsrEvent>,
    /// 每收到多少帧音频吐一个脚本事件
    pub frames_per_event: usize,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            latency: Duration::ZERO,
            fail: false,
            fail_every: None,
            hang: false,
            hotword: None,
            script: Vec::new(),
            frames_per_event: 5,
        }
    }
}

fn should_fail(cfg: &MockConfig, call: usize) -> bool {
    if cfg.fail {
        return true;
    }
    match cfg.fail_every {
        // 第 n、2n、3n… 次调用失败（call 从 0 开始计数）
        Some(n) if n >= 2 => ((call as u32) + 1).is_multiple_of(n),
        _ => false,
    }
}

/// 假 AI：上下文相关性打分。
///
/// - 候选整词出现在光标前文里 → 大幅加分
/// - 候选的相邻二字组出现在前文里 → 小幅加分
///
/// 这只是「有上下文的排序」的最小可运行版本，真实实现由 LLM 供应商替换。
fn context_score(text: &str, ctx: &ContextSnapshot) -> f32 {
    let before = ctx.text_before.as_str();
    if before.is_empty() || text.is_empty() {
        return 0.0;
    }
    let mut score = 0.0;
    if before.contains(text) {
        score += 10.0;
    }
    let chars: Vec<char> = text.chars().collect();
    for w in chars.windows(2) {
        let gram: String = w.iter().collect();
        if before.contains(&gram) {
            score += 1.5;
        }
    }
    score
}

#[derive(Debug)]
pub struct MockLlmReranker {
    cfg: Arc<MockConfig>,
    calls: AtomicUsize,
}

impl MockLlmReranker {
    pub fn new(cfg: MockConfig) -> Self {
        Self {
            cfg: Arc::new(cfg),
            calls: AtomicUsize::new(0),
        }
    }

    pub fn new_shared(cfg: MockConfig) -> (Self, Arc<MockConfig>) {
        let c = Arc::new(cfg);
        (
            Self {
                cfg: Arc::clone(&c),
                calls: AtomicUsize::new(0),
            },
            c,
        )
    }

    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl LlmReranker for MockLlmReranker {
    fn rerank(&self, req: RerankRequest) -> BoxFuture<'static, Result<RerankResponse, CloudError>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let cfg = Arc::clone(&self.cfg);
        Box::pin(async move {
            if cfg.hang {
                // 永不就绪：验证外层超时能否真的把这次调用丢掉
                return futures::future::pending::<Result<RerankResponse, CloudError>>().await;
            }
            if !cfg.latency.is_zero() {
                // mock 专用：真实实现必须用异步定时器，不能睡线程
                std::thread::sleep(cfg.latency);
            }
            if should_fail(&cfg, call) {
                return Err(CloudError::Network(format!("mock 第 {call} 次调用失败")));
            }

            let n = req.candidates.len();
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by(|&a, &b| {
                let sa = req
                    .candidates
                    .get(a)
                    .map(|c| context_score(&c.text, &req.context))
                    .unwrap_or(0.0);
                let sb = req
                    .candidates
                    .get(b)
                    .map(|c| context_score(&c.text, &req.context))
                    .unwrap_or(0.0);
                sb.partial_cmp(&sa)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.cmp(&b))
            });

            let mut extra = Vec::new();
            if let Some(hw) = &cfg.hotword {
                extra.push(Candidate {
                    language: Default::default(),
                    text: hw.clone(),
                    comment: "热词".into(),
                    source: CandidateSource::Hotword,
                    syllable_len: req.syllables.len(),
                    consumed: req.composition.len(),
                    syllables: Vec::new(),
                    score: 0.0,
                });
            }
            Ok(RerankResponse {
                order,
                extra,
                polished: None,
            })
        })
    }
}

#[derive(Debug)]
pub struct MockCloudPinyin {
    cfg: Arc<MockConfig>,
    calls: AtomicUsize,
}

impl MockCloudPinyin {
    pub fn new(cfg: MockConfig) -> Self {
        Self {
            cfg: Arc::new(cfg),
            calls: AtomicUsize::new(0),
        }
    }
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl CloudPinyin for MockCloudPinyin {
    fn suggest(
        &self,
        _req: PinyinRequest,
    ) -> BoxFuture<'static, Result<PinyinSuggestion, CloudError>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let cfg = Arc::clone(&self.cfg);
        Box::pin(async move {
            if cfg.hang {
                return futures::future::pending::<Result<PinyinSuggestion, CloudError>>().await;
            }
            if !cfg.latency.is_zero() {
                std::thread::sleep(cfg.latency);
            }
            if should_fail(&cfg, call) {
                return Err(CloudError::Network("mock 云拼音失败".into()));
            }
            Ok(PinyinSuggestion {
                sentences: Vec::new(),
                hotwords: cfg.hotword.iter().cloned().collect(),
            })
        })
    }
}

struct MockSession {
    cfg: Arc<MockConfig>,
    script: VecDeque<AsrEvent>,
    pending: VecDeque<AsrEvent>,
    frames: usize,
    finished: bool,
    cancelled: bool,
}

impl AsrSession for MockSession {
    fn push_audio(&mut self, frame: &[u8]) -> Result<(), CloudError> {
        if self.cancelled {
            return Err(CloudError::Unavailable);
        }
        if frame.is_empty() {
            return Ok(());
        }
        self.frames += 1;
        let every = self.cfg.frames_per_event.max(1);
        if self.frames.is_multiple_of(every) {
            if let Some(ev) = self.script.pop_front() {
                self.pending.push_back(ev);
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), CloudError> {
        if self.cancelled {
            return Err(CloudError::Unavailable);
        }
        // test.md 第二节：松手后必须把剩下的（含 final）全部要回来
        self.pending.extend(self.script.drain(..));
        self.finished = true;
        Ok(())
    }

    fn poll(&mut self) -> Vec<AsrEvent> {
        self.pending.drain(..).collect()
    }

    fn is_done(&self) -> bool {
        self.finished && self.pending.is_empty()
    }

    fn cancel(&mut self) {
        self.cancelled = true;
        self.script.clear();
        self.pending.clear();
    }
}

#[derive(Debug)]
pub struct MockAsr {
    cfg: Arc<MockConfig>,
}

impl MockAsr {
    pub fn new(cfg: MockConfig) -> Self {
        Self { cfg: Arc::new(cfg) }
    }
}

impl StreamingAsr for MockAsr {
    fn start(&self, _cfg: AsrConfig) -> Result<Box<dyn AsrSession>, CloudError> {
        if self.cfg.fail {
            return Err(CloudError::Unavailable);
        }
        Ok(Box::new(MockSession {
            cfg: Arc::clone(&self.cfg),
            script: self.cfg.script.iter().cloned().collect(),
            pending: VecDeque::new(),
            frames: 0,
            finished: false,
            cancelled: false,
        }))
    }
}

/// 「没有配置云端」时的实现：三个 trait 全部立即失败。
///
/// 这是默认装配 —— 用户没填任何云端凭据时，输入法必须完全靠本地工作。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableCloud;

impl CloudPinyin for UnavailableCloud {
    fn suggest(
        &self,
        _req: PinyinRequest,
    ) -> BoxFuture<'static, Result<PinyinSuggestion, CloudError>> {
        Box::pin(async { Err(CloudError::Unavailable) })
    }
}

impl LlmReranker for UnavailableCloud {
    fn rerank(
        &self,
        _req: RerankRequest,
    ) -> BoxFuture<'static, Result<RerankResponse, CloudError>> {
        Box::pin(async { Err(CloudError::Unavailable) })
    }
}

impl StreamingAsr for UnavailableCloud {
    fn start(&self, _cfg: AsrConfig) -> Result<Box<dyn AsrSession>, CloudError> {
        Err(CloudError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use retype_types::{InputSource, PrivacyLevel};

    fn candidate(text: &str) -> Candidate {
        Candidate::new(text, CandidateSource::Local)
    }

    fn rerank_req(cands: Vec<&str>, before: &str) -> RerankRequest {
        RerankRequest {
            composition: "shili".into(),
            syllables: vec!["shi".into(), "li".into()],
            candidates: cands.into_iter().map(candidate).collect(),
            context: ContextSnapshot {
                text_before: before.into(),
                privacy: PrivacyLevel::Cloud,
                ..Default::default()
            },
            source: InputSource::Keyboard,
        }
    }

    #[test]
    fn context_boosts_relevant_candidate() {
        let m = MockLlmReranker::new(MockConfig::default());
        let req = rerank_req(vec!["实力", "事例", "治理"], "我们需要提升团队的技术实力");
        let resp = futures::executor::block_on(m.rerank(req)).unwrap();
        assert_eq!(
            resp.order.first().copied(),
            Some(0),
            "上下文里出现过的词应被提前"
        );
    }

    #[test]
    fn without_context_order_is_preserved() {
        let m = MockLlmReranker::new(MockConfig::default());
        let req = rerank_req(vec!["实力", "事例", "治理"], "");
        let resp = futures::executor::block_on(m.rerank(req)).unwrap();
        assert_eq!(resp.order, vec![0, 1, 2], "无上下文时二刷不应改变顺序");
    }

    #[test]
    fn hotword_lands_in_extra_not_in_order() {
        let m = MockLlmReranker::new(MockConfig {
            hotword: Some("豆包输入法".into()),
            ..Default::default()
        });
        let resp = futures::executor::block_on(m.rerank(rerank_req(vec!["实力"], ""))).unwrap();
        assert_eq!(resp.order, vec![0], "云端热词不许冒充本地候选（P3）");
        assert_eq!(resp.extra.len(), 1);
        assert_eq!(resp.extra[0].source, CandidateSource::Hotword);
    }

    #[test]
    fn fail_every_is_deterministic() {
        let m = MockLlmReranker::new(MockConfig {
            fail_every: Some(2),
            ..Default::default()
        });
        let r0 = futures::executor::block_on(m.rerank(rerank_req(vec!["a"], "")));
        let r1 = futures::executor::block_on(m.rerank(rerank_req(vec!["a"], "")));
        assert!(r0.is_ok());
        assert!(r1.is_err(), "fail_every=2 时第 2 次调用（call=1）应失败");
    }

    #[test]
    fn asr_script_replays_three_passes() {
        let m = MockAsr::new(MockConfig {
            frames_per_event: 1,
            script: vec![
                AsrEvent::Interim("今天天".into()),
                AsrEvent::Interim("今天天气".into()),
                AsrEvent::Stable("今天天气不错".into()),
                AsrEvent::Final("今天天气不错。".into()),
            ],
            ..Default::default()
        });
        let mut s = m.start(AsrConfig::default()).unwrap();
        s.push_audio(&[0u8; 320]).unwrap();
        assert_eq!(s.poll(), vec![AsrEvent::Interim("今天天".into())]);
        s.push_audio(&[0u8; 320]).unwrap();
        assert!(!s.is_done());
        // 松手：剩下的事件（含 final）必须全部到齐
        s.finish().unwrap();
        let rest = s.poll();
        assert!(
            rest.iter().any(|e| matches!(e, AsrEvent::Final(_))),
            "{rest:?}"
        );
        assert!(s.is_done());
    }

    #[test]
    fn cancelled_session_rejects_audio() {
        let m = MockAsr::new(MockConfig::default());
        let mut s = m.start(AsrConfig::default()).unwrap();
        s.cancel();
        assert!(s.push_audio(&[1, 2, 3]).is_err());
    }

    #[test]
    fn unavailable_cloud_fails_fast() {
        let c = UnavailableCloud;
        let e = futures::executor::block_on(LlmReranker::rerank(&c, rerank_req(vec!["a"], "")))
            .unwrap_err();
        assert_eq!(e, CloudError::Unavailable);
        assert!(StreamingAsr::start(&c, AsrConfig::default()).is_err());
    }
}
