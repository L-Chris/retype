//! 内核后端：把「纯状态机」接上并发。
//!
//! [`Kernel`] 本身不开线程、不做 IO，所以「二刷在哪个线程跑、学习什么时候落盘、
//! 超时谁来兜底」全部是后端的事。两个实现：
//!
//! - [`LocalBackend`]：进程内 worker 线程（M0–M2 的生产形态，见 ADR-0004）
//! - [`InlineBackend`]：同步执行云端副作用（单元测试与 `retype-diag --inline` 用，
//!   一次 `submit` 就能跑完「首刷 → 二刷 → 合并」整条链，结果完全确定）
//!
//! M3 抽出 `retype-host.exe` 时，只需新增一个 `RemoteBackend`，上层零改动。

use crate::kernel::Kernel;
use crate::merge::apply_response;
use crossbeam_channel::{unbounded, Receiver, Sender};
use retype_cloud::{CloudClient, CloudResult, RerankRequest};
use retype_types::{
    InputEvent, KernelAction, LearningEvent, RenderState, RerankJob, RerankOutcome, SideEffect,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

/// 后端契约。
pub trait KernelBackend: Send + Sync {
    /// 同步处理事件，返回**必须立即执行**的动作（渲染 / 上屏）。
    ///
    /// 实现必须保证耗时只包含首刷（预算 < 5ms）；二刷、学习落盘一律转 worker。
    /// 这条约束就是 P1（输入主线程绝不阻塞）的可执行形式。
    fn submit(&self, ev: InputEvent) -> Vec<KernelAction>;

    /// 非阻塞取出异步产生的动作（例如二刷完成后的重渲染）。
    /// UI 线程在自己的消息循环里轮询它。
    fn poll_action(&self) -> Option<KernelAction>;

    /// 取一份当前渲染状态快照。
    ///
    /// 刻意不暴露 `with_kernel<R>(impl FnOnce(&Kernel) -> R)`：
    /// 带泛型方法的 trait 不是 dyn-compatible 的，`Arc<dyn KernelBackend>` 就没法用了，
    /// 而「平台层只依赖 trait object」正是 M3 能无痛换成 RemoteBackend 的前提。
    fn render(&self) -> RenderState;
}

fn lock_or_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // 中毒恢复：worker 线程 panic 不能让键盘失灵（P2）
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// 把二刷任务变成结果。两个后端共用，保证「降级 == 空结果」的语义只有一份。
fn run_rerank(cloud: &CloudClient, job: &RerankJob) -> RerankOutcome {
    let req = RerankRequest {
        composition: job.composition.clone(),
        syllables: job.syllables.clone(),
        candidates: job.candidates.clone(),
        context: job.context.clone(),
        source: job.source,
    };
    match cloud.rerank(req) {
        CloudResult::Ok(resp) => apply_response(&job.candidates, &resp),
        CloudResult::Degraded(_) => RerankOutcome {
            ranked: Vec::new(),
            extra: Vec::new(),
            degraded: true,
        },
    }
}

/// 降级用的空结果：二刷失败时首刷候选原样保留（P3）。
fn degraded_outcome() -> RerankOutcome {
    RerankOutcome {
        ranked: Vec::new(),
        extra: Vec::new(),
        degraded: true,
    }
}

/// TIP 跑在宿主进程里，任何 worker panic 都不能冒泡出去。
/// `AssertUnwindSafe` 在这里是安全的：worker 只读内核状态，panic 后锁会被
/// `lock_or_recover` 从中毒状态恢复，最坏结果是这一次二刷/学习丢失。
fn guarded<F, R>(what: &'static str, fallback: R, f: F) -> R
where
    F: FnOnce() -> R,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            tracing::error!("{what} worker panic，已吞掉（不影响键盘）");
            fallback
        }
    }
}

// ─────────────────────────── 同步后端 ───────────────────────────

/// 同步后端：云端副作用在 `submit` 里就地执行。
///
/// 只用于测试与诊断工具 —— 生产环境下它会把云端延迟搬到输入线程上，直接违反 P1。
pub struct InlineBackend {
    kernel: Mutex<Kernel>,
    cloud: Arc<CloudClient>,
    /// 事件级联的步数上限，防止 RerankCompleted 触发新的 Rerank 造成死循环
    max_steps: usize,
}

impl std::fmt::Debug for InlineBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InlineBackend").finish_non_exhaustive()
    }
}

impl InlineBackend {
    pub fn new(kernel: Kernel, cloud: Arc<CloudClient>) -> Self {
        Self {
            kernel: Mutex::new(kernel),
            cloud,
            max_steps: 32,
        }
    }

    /// 直接访问内核。只对**具体类型**开放，不进 trait（见 `KernelBackend::render` 的注释）。
    pub fn with_kernel<R>(&self, f: impl FnOnce(&Kernel) -> R) -> R {
        f(&lock_or_recover(&self.kernel))
    }
}

impl KernelBackend for InlineBackend {
    fn submit(&self, ev: InputEvent) -> Vec<KernelAction> {
        let mut pending: VecDeque<InputEvent> = VecDeque::new();
        pending.push_back(ev);
        let mut out = Vec::new();
        let mut steps = 0;

        while let Some(e) = pending.pop_front() {
            steps += 1;
            if steps > self.max_steps {
                tracing::warn!("InlineBackend 事件级联超过 {} 步，已截断", self.max_steps);
                break;
            }
            let acts = { lock_or_recover(&self.kernel).handle(e) };
            for a in acts {
                match a {
                    KernelAction::Side(SideEffect::Rerank(job)) => {
                        let outcome = run_rerank(&self.cloud, &job);
                        pending.push_back(InputEvent::RerankCompleted {
                            gen: job.gen,
                            result: outcome,
                        });
                    }
                    KernelAction::Side(SideEffect::Learn(lev)) => {
                        let learner = { Arc::clone(lock_or_recover(&self.kernel).learner()) };
                        learner.record(lev);
                    }
                    // 上下文采集只有平台层能做，必须原样转交
                    other => out.push(other),
                }
            }
        }
        out
    }

    fn poll_action(&self) -> Option<KernelAction> {
        None
    }

    fn render(&self) -> RenderState {
        lock_or_recover(&self.kernel).render_state()
    }
}

// ─────────────────────────── 异步后端 ───────────────────────────

#[derive(Debug, Clone)]
pub struct BackendOptions {
    /// 二刷去抖窗口。
    ///
    /// 用户连续打字时，每一次按键都会产生一个新的二刷任务，而前面那些**必然过期**
    /// （gen 已经落后）。与其发出去再丢弃，不如等一个停顿：
    /// 窗口内不断有新任务就用最新的替换旧的，只在真正停手时才发一次请求。
    /// 这样既省掉无谓的网络往返，也避免了「积压的请求在停手后一起回来搅乱候选」。
    pub rerank_debounce: Duration,
}

impl Default for BackendOptions {
    fn default() -> Self {
        Self {
            // 120ms：比正常打字的按键间隔略长，比人眼可察觉的延迟短
            rerank_debounce: Duration::from_millis(120),
        }
    }
}

/// 进程内异步后端（M0–M2 的生产形态，ADR-0004）。
///
/// 线程布局：
/// - 调用方线程（TSF 主线程）：只做首刷 + 投递，立即返回
/// - `retype-rerank`：单线程，去抖后串行执行二刷
/// - `retype-learn`：单线程，串行落盘学习事件
///
/// 刻意用「单线程 + 队列」而不是线程池：二刷和学习都是幂等的顺序任务，
/// 线程池只会带来乱序（旧结果覆盖新结果）和无谓的上下文切换。
pub struct LocalBackend {
    kernel: Mutex<Kernel>,
    cloud: Arc<CloudClient>,
    opts: BackendOptions,
    out: Sender<KernelAction>,
    rx: Receiver<KernelAction>,
    jobs: Sender<RerankJob>,
    learns: Sender<LearningEvent>,
    me: OnceLock<Weak<Self>>,
}

impl std::fmt::Debug for LocalBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalBackend")
            .field("opts", &self.opts)
            .finish_non_exhaustive()
    }
}

impl LocalBackend {
    pub fn new(kernel: Kernel, cloud: Arc<CloudClient>) -> Arc<Self> {
        Self::with_options(kernel, cloud, BackendOptions::default())
    }

    pub fn with_options(
        kernel: Kernel,
        cloud: Arc<CloudClient>,
        opts: BackendOptions,
    ) -> Arc<Self> {
        let (out, rx) = unbounded();
        let (jobs_tx, jobs_rx) = unbounded::<RerankJob>();
        let (learn_tx, learn_rx) = unbounded::<LearningEvent>();
        let b = Arc::new(Self {
            kernel: Mutex::new(kernel),
            cloud,
            opts: opts.clone(),
            out,
            rx,
            jobs: jobs_tx,
            learns: learn_tx,
            me: OnceLock::new(),
        });
        let _ = b.me.set(Arc::downgrade(&b));
        spawn_rerank_worker(Arc::downgrade(&b), jobs_rx, opts.rerank_debounce);
        spawn_learn_worker(Arc::downgrade(&b), learn_rx);
        b
    }

    /// 队列深度，诊断用（持续 > 0 说明二刷跟不上打字速度）。
    pub fn pending_jobs(&self) -> usize {
        self.jobs.len()
    }

    /// 直接访问内核。只对**具体类型**开放，不进 trait（见 `KernelBackend::render` 的注释）。
    pub fn with_kernel<R>(&self, f: impl FnOnce(&Kernel) -> R) -> R {
        f(&lock_or_recover(&self.kernel))
    }
}

/// 二刷 worker：去抖 + 串行执行 + 把结果回灌内核。
fn spawn_rerank_worker(weak: Weak<LocalBackend>, rx: Receiver<RerankJob>, debounce: Duration) {
    let spawned = std::thread::Builder::new()
        .name("retype-rerank".into())
        .spawn(move || {
            // 1. 阻塞等第一个任务；通道断开（LocalBackend 被释放）时自然退出，不泄漏线程
            while let Ok(mut job) = rx.recv() {
                // 2. 去抖：窗口内只要来了新任务，就用新的替换旧的（旧的必然已过期）。
                // 三态匹配是刻意的：Disconnected 必须终止整个 worker，
                // 写成 `while let Ok(..)` 会把它和 Timeout 混为一谈，导致线程空转。
                #[allow(clippy::while_let_loop)]
                loop {
                    match rx.recv_timeout(debounce) {
                        Ok(newer) => job = newer,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                    }
                }
                let Some(b) = weak.upgrade() else { break };
                let gen = job.gen;
                let cloud = Arc::clone(&b.cloud);
                let outcome = guarded("rerank", degraded_outcome(), || run_rerank(&cloud, &job));
                let acts = {
                    lock_or_recover(&b.kernel).handle(InputEvent::RerankCompleted {
                        gen,
                        result: outcome,
                    })
                };
                for a in acts {
                    let _ = b.out.send(a);
                }
            }
        });
    if spawned.is_err() {
        tracing::error!("无法启动二刷 worker，二刷将被禁用（首刷不受影响）");
    }
}

/// 学习 worker：串行落盘，绝不占用输入线程。
fn spawn_learn_worker(weak: Weak<LocalBackend>, rx: Receiver<LearningEvent>) {
    let spawned = std::thread::Builder::new()
        .name("retype-learn".into())
        .spawn(move || {
            while let Ok(ev) = rx.recv() {
                let Some(b) = weak.upgrade() else { break };
                let learner = { Arc::clone(lock_or_recover(&b.kernel).learner()) };
                guarded("learn", (), move || learner.record(ev));
            }
        });
    if spawned.is_err() {
        tracing::error!("无法启动学习 worker，本次会话不会积累用户词");
    }
}

impl KernelBackend for LocalBackend {
    fn submit(&self, ev: InputEvent) -> Vec<KernelAction> {
        // 锁只在这一次纯计算期间持有，绝不跨调用（P1）
        let acts = { lock_or_recover(&self.kernel).handle(ev) };
        let mut out = Vec::with_capacity(acts.len());
        for a in acts {
            match a {
                KernelAction::Side(SideEffect::Rerank(job)) => {
                    // 投递失败（worker 已退出）就静默放弃二刷，首刷结果照常可用
                    let _ = self.jobs.send(job);
                }
                KernelAction::Side(SideEffect::Learn(lev)) => {
                    let _ = self.learns.send(lev);
                }
                other => out.push(other),
            }
        }
        out
    }

    fn poll_action(&self) -> Option<KernelAction> {
        self.rx.try_recv().ok()
    }

    fn render(&self) -> RenderState {
        lock_or_recover(&self.kernel).render_state()
    }
}
