//! 云端调用外壳：隐私闸门 + 超时 + 熔断 + 线程隔离。
//!
//! 这一层的存在意义就是把 [ARCHITECTURE.md §7 降级矩阵](../../../ARCHITECTURE.md#7-降级矩阵)
//! 变成代码里绕不过去的路径：上层（内核）调用 `rerank` **永远不会失败**，
//! 只会拿到 `Ok(结果)` 或 `Degraded(原因)`，因此不可能出现「网络错误把键盘卡住」。

use crate::breaker::{BreakerState, CircuitBreaker};
use crate::{
    CloudError, CloudPinyin, LlmReranker, PinyinRequest, PinyinSuggestion, RerankRequest,
    RerankResponse,
};
use futures::future::BoxFuture;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// 云端调用结果。刻意不用 `Result`，避免上层写 `?` 把降级当成错误传播出去。
#[derive(Debug, Clone, PartialEq)]
pub enum CloudResult<T> {
    Ok(T),
    Degraded(CloudError),
}

impl<T> CloudResult<T> {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok(_))
    }

    pub fn into_ok(self) -> Option<T> {
        match self {
            Self::Ok(v) => Some(v),
            Self::Degraded(_) => None,
        }
    }

    pub fn reason(&self) -> Option<&CloudError> {
        match self {
            Self::Ok(_) => None,
            Self::Degraded(e) => Some(e),
        }
    }
}

/// 在独立线程上跑 future 并施加超时。
///
/// 超时后**放弃等待但不杀线程**：Rust 无法安全地终止线程，泄漏的那次调用会自己跑完。
/// 因此 trait 契约要求实现方自带内部超时（`CloudClient` 的超时只是最后一道保险）。
fn run_with_timeout<R: Send + 'static>(
    timeout: Duration,
    make: impl FnOnce() -> BoxFuture<'static, R> + Send + 'static,
) -> Result<R, CloudError> {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new()
        .name("retype-cloud".into())
        .spawn(move || {
            let v = futures::executor::block_on(make());
            let _ = tx.send(v);
        });
    if spawned.is_err() {
        return Err(CloudError::Unavailable);
    }
    match rx.recv_timeout(timeout) {
        Ok(v) => Ok(v),
        Err(RecvTimeoutError::Timeout) => Err(CloudError::Timeout),
        Err(RecvTimeoutError::Disconnected) => {
            Err(CloudError::Network("云端工作线程意外退出".into()))
        }
    }
}

/// 云端客户端。所有方法都是**阻塞**的，只允许在 worker 线程上调用（P1）。
pub struct CloudClient {
    reranker: Arc<dyn LlmReranker>,
    pinyin: Arc<dyn CloudPinyin>,
    timeout: Duration,
    breaker: Mutex<CircuitBreaker>,
}

impl std::fmt::Debug for CloudClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudClient")
            .field("timeout", &self.timeout)
            .field("breaker", &self.breaker_state())
            .finish()
    }
}

impl CloudClient {
    pub fn new(
        reranker: Arc<dyn LlmReranker>,
        pinyin: Arc<dyn CloudPinyin>,
        timeout: Duration,
    ) -> Self {
        Self {
            reranker,
            pinyin,
            timeout,
            breaker: Mutex::new(CircuitBreaker::default_policy()),
        }
    }

    fn breaker(&self) -> MutexGuard<'_, CircuitBreaker> {
        // 中毒恢复：熔断器状态丢了不该让键盘失灵
        match self.breaker.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    pub fn breaker_state(&self) -> BreakerState {
        self.breaker().state()
    }

    /// 云端当前是否值得尝试。用于点亮/熄灭状态栏的「云」图标。
    pub fn is_available(&self) -> bool {
        self.breaker_state() != BreakerState::Open
    }

    pub fn reset_breaker(&self) {
        self.breaker().reset();
    }

    /// 二刷：上下文重排。
    pub fn rerank(&self, mut req: RerankRequest) -> CloudResult<RerankResponse> {
        // 隐私闸门：不管调用方传了什么，出网前一律重新盖章
        req.context = req.context.sanitized_for_cloud();

        if !self.breaker().allow() {
            return CloudResult::Degraded(CloudError::CircuitOpen);
        }

        let r = Arc::clone(&self.reranker);
        let res = run_with_timeout(self.timeout, move || r.rerank(req));
        match res {
            Ok(Ok(resp)) => {
                self.breaker().record_success();
                CloudResult::Ok(resp)
            }
            Ok(Err(e)) => {
                self.breaker().record_failure();
                CloudResult::Degraded(e)
            }
            Err(e) => {
                self.breaker().record_failure();
                CloudResult::Degraded(e)
            }
        }
    }

    /// 云端整句 / 热词。
    pub fn suggest(&self, mut req: PinyinRequest) -> CloudResult<PinyinSuggestion> {
        req.context = req.context.sanitized_for_cloud();
        if !self.breaker().allow() {
            return CloudResult::Degraded(CloudError::CircuitOpen);
        }
        let p = Arc::clone(&self.pinyin);
        match run_with_timeout(self.timeout, move || p.suggest(req)) {
            Ok(Ok(v)) => {
                self.breaker().record_success();
                CloudResult::Ok(v)
            }
            Ok(Err(e)) | Err(e) => {
                self.breaker().record_failure();
                CloudResult::Degraded(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::mock::{MockCloudPinyin, MockConfig, MockLlmReranker};
    use retype_types::{Candidate, CandidateSource, ContextSnapshot, InputSource, PrivacyLevel};

    fn req(n: usize) -> RerankRequest {
        RerankRequest {
            composition: "nihao".into(),
            syllables: vec!["ni".into(), "hao".into()],
            candidates: (0..n)
                .map(|i| Candidate::new(format!("候选{i}"), CandidateSource::Local))
                .collect(),
            context: ContextSnapshot {
                text_before: "上下文".into(),
                privacy: PrivacyLevel::Cloud,
                ..Default::default()
            },
            source: InputSource::Keyboard,
        }
    }

    fn client(cfg: MockConfig, timeout: Duration) -> CloudClient {
        CloudClient::new(
            Arc::new(MockLlmReranker::new(cfg.clone())),
            Arc::new(MockCloudPinyin::new(cfg)),
            timeout,
        )
    }

    #[test]
    fn happy_path_returns_response() {
        let c = client(MockConfig::default(), Duration::from_secs(2));
        let r = c.rerank(req(3));
        assert!(r.is_ok());
        assert_eq!(c.breaker_state(), BreakerState::Closed);
    }

    #[test]
    fn timeout_degrades_instead_of_blocking_forever() {
        // 这是 test.md「云端超时时不影响键盘」的最小复现
        let cfg = MockConfig {
            latency: Duration::from_millis(500),
            ..Default::default()
        };
        let c = client(cfg, Duration::from_millis(30));
        let start = std::time::Instant::now();
        let r = c.rerank(req(3));
        assert!(matches!(r, CloudResult::Degraded(CloudError::Timeout)));
        assert!(
            start.elapsed() < Duration::from_millis(400),
            "必须在超时点返回，而不是等云端跑完"
        );
    }

    #[test]
    fn repeated_failures_open_the_breaker() {
        let cfg = MockConfig {
            fail: true,
            ..Default::default()
        };
        let c = client(cfg, Duration::from_secs(1));
        for _ in 0..5 {
            assert!(matches!(c.rerank(req(1)), CloudResult::Degraded(_)));
        }
        assert_eq!(c.breaker_state(), BreakerState::Open);
        assert!(!c.is_available());
        // 熔断后应立即返回 CircuitOpen，不再消耗一次超时等待
        let start = std::time::Instant::now();
        assert!(matches!(
            c.rerank(req(1)),
            CloudResult::Degraded(CloudError::CircuitOpen)
        ));
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn context_is_stripped_before_leaving_the_process() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Spy {
            seen: Arc<Mutex<Vec<String>>>,
            calls: AtomicUsize,
        }
        impl LlmReranker for Spy {
            fn rerank(
                &self,
                req: RerankRequest,
            ) -> BoxFuture<'static, Result<RerankResponse, CloudError>> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let before = req.context.text_before.clone();
                let pkg = req.context.app.package.clone();
                let seen = Arc::clone(&self.seen);
                Box::pin(async move {
                    if let Ok(mut g) = seen.lock() {
                        g.push(format!("{pkg}|{before}"));
                    }
                    Ok(RerankResponse::default())
                })
            }
        }

        let seen = Arc::new(Mutex::new(Vec::new()));
        let c = CloudClient::new(
            Arc::new(Spy {
                seen: Arc::clone(&seen),
                calls: AtomicUsize::new(0),
            }),
            Arc::new(MockCloudPinyin::new(MockConfig::default())),
            Duration::from_secs(2),
        );

        let mut r = req(1);
        // 采集方盖章为 Local：不允许出网
        r.context.privacy = PrivacyLevel::Local;
        r.context.text_before = "我的银行卡密码".into();
        r.context.app.package = "notepad.exe".into();
        c.rerank(r);

        let g = seen.lock().unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(
            g[0], "|",
            "上下文与进程名都必须在出网前被清空，实际收到 {:?}",
            g[0]
        );
    }
}
