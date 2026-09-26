//! 熔断器。
//!
//! test.md 第九节：*"断网时仍能输入、云端超时时不影响键盘"*。
//! 没有熔断器的话，云端 hang 死时每次按键都会白等一个超时 —— 用户会感觉键盘变钝，
//! 这比完全没有云端能力更糟。

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// 正常放行
    Closed,
    /// 拒绝请求，直到冷却期结束
    Open,
    /// 冷却期结束，放行一个探针请求
    HalfOpen,
}

#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    state: BreakerState,
    failures: u32,
    opened_at: Option<Instant>,
    threshold: u32,
    cooldown: Duration,
}

impl CircuitBreaker {
    pub fn new(threshold: u32, cooldown: Duration) -> Self {
        Self {
            state: BreakerState::Closed,
            failures: 0,
            opened_at: None,
            threshold: threshold.max(1),
            cooldown,
        }
    }

    /// 常见默认：连续 5 次失败后熔断 30 秒。
    pub fn default_policy() -> Self {
        Self::new(5, Duration::from_secs(30))
    }

    pub fn state(&self) -> BreakerState {
        self.state
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// 是否放行本次请求。会顺带完成 Open → HalfOpen 的状态迁移。
    pub fn allow(&mut self) -> bool {
        self.allow_at(Instant::now())
    }

    pub fn allow_at(&mut self, now: Instant) -> bool {
        match self.state {
            BreakerState::Closed => true,
            BreakerState::HalfOpen => true,
            BreakerState::Open => {
                let elapsed = self.opened_at.map(|t| now.saturating_duration_since(t));
                if elapsed.is_some_and(|e| e >= self.cooldown) {
                    self.state = BreakerState::HalfOpen;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn record_success(&mut self) {
        self.record_success_at(Instant::now());
    }

    pub fn record_success_at(&mut self, _now: Instant) {
        self.failures = 0;
        self.state = BreakerState::Closed;
        self.opened_at = None;
    }

    pub fn record_failure(&mut self) {
        self.record_failure_at(Instant::now());
    }

    pub fn record_failure_at(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1);
        // HalfOpen 下的探针失败要立刻重新熔断，并且冷却时间翻倍（指数退避，上限 5 分钟）
        let was_half_open = self.state == BreakerState::HalfOpen;
        if was_half_open {
            self.cooldown = (self.cooldown * 2).min(Duration::from_secs(300));
        }
        if was_half_open || self.failures >= self.threshold {
            self.state = BreakerState::Open;
            self.opened_at = Some(now);
            self.failures = 0;
        }
    }

    /// 手动复位（设置界面里的「重新连接云端」）。
    pub fn reset(&mut self) {
        self.state = BreakerState::Closed;
        self.failures = 0;
        self.opened_at = None;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn t(offset_ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(offset_ms)
    }

    #[test]
    fn closed_until_threshold() {
        let mut b = CircuitBreaker::new(3, Duration::from_secs(1));
        for i in 0..2 {
            b.record_failure_at(t(i));
            assert!(b.allow_at(t(i)), "还没到阈值就该放行");
        }
        assert_eq!(b.state(), BreakerState::Closed);
        b.record_failure_at(t(2));
        assert_eq!(b.state(), BreakerState::Open);
        assert!(
            !b.allow_at(t(2)),
            "熔断后必须拒绝，否则每次按键都白等一个超时"
        );
    }

    #[test]
    fn half_open_after_cooldown_then_recovers() {
        let mut b = CircuitBreaker::new(1, Duration::from_millis(100));
        b.record_failure_at(t(0));
        assert!(!b.allow_at(t(50)));
        assert!(b.allow_at(t(150)), "冷却结束应放行探针");
        assert_eq!(b.state(), BreakerState::HalfOpen);
        b.record_success_at(t(160));
        assert_eq!(b.state(), BreakerState::Closed);
        assert!(b.allow_at(t(160)));
    }

    #[test]
    fn failed_probe_extends_cooldown() {
        let mut b = CircuitBreaker::new(1, Duration::from_millis(100));
        b.record_failure_at(t(0));
        assert!(b.allow_at(t(150)));
        b.record_failure_at(t(160)); // 探针又失败
        assert_eq!(b.state(), BreakerState::Open);
        assert!(!b.allow_at(t(250)), "冷却已翻倍到 200ms，此时仍应拒绝");
        assert!(b.allow_at(t(400)));
    }

    #[test]
    fn success_clears_failure_count() {
        let mut b = CircuitBreaker::new(3, Duration::from_secs(1));
        b.record_failure_at(t(0));
        b.record_failure_at(t(1));
        b.record_success_at(t(2));
        b.record_failure_at(t(3));
        b.record_failure_at(t(4));
        assert_eq!(b.state(), BreakerState::Closed, "成功后计数应清零");
    }
}
