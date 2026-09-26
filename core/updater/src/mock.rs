//! 测试用的 HTTP 桩。
//!
//! 和 `retype-cloud` 的 Mock 同一个思路（ADR-0002）：
//! **桩的价值在于能主动制造故障**。404、限流、500、传输层断连、
//! 返回一段坏 JSON —— 这些都必须在不联网的情况下可测，
//! 否则「更新检查失败会不会把设置界面卡住」这类问题只能等上线后才知道。

use crate::{HttpFetcher, Response, UpdateError};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct MockHttp {
    routes: HashMap<String, Response>,
    transport_error: Option<String>,
    /// 未命中路由时返回的状态码，默认 404（等价于「还没发布过任何版本」）
    fallback_status: u16,
    calls: Mutex<Vec<String>>,
}

impl MockHttp {
    pub fn new() -> Self {
        Self {
            fallback_status: 404,
            ..Default::default()
        }
    }

    /// 注册一个返回 JSON 的路由。
    pub fn with_json(mut self, url: &str, status: u16, body: &str) -> Self {
        self.routes.insert(
            url.to_owned(),
            Response {
                status,
                body: body.as_bytes().to_vec(),
            },
        );
        self
    }

    /// 注册一个只有状态码、没有 body 的路由（模拟限流/服务端错误）。
    pub fn with_status(mut self, url: &str, status: u16) -> Self {
        self.routes.insert(
            url.to_owned(),
            Response {
                status,
                body: Vec::new(),
            },
        );
        self
    }

    /// 模拟传输层失败（DNS 挂了、断网、TLS 握手失败）。
    pub fn with_transport_error(mut self, msg: &str) -> Self {
        self.transport_error = Some(msg.to_owned());
        self
    }

    pub fn with_fallback_status(mut self, status: u16) -> Self {
        self.fallback_status = status;
        self
    }

    /// 实际请求过的 URL，用于断言「没有多余请求」和「请求了正确的地址」。
    pub fn calls(&self) -> Vec<String> {
        match self.calls.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    pub fn call_count(&self) -> usize {
        self.calls().len()
    }
}

impl HttpFetcher for MockHttp {
    fn get(&self, url: &str) -> Result<Response, UpdateError> {
        if let Ok(mut g) = self.calls.lock() {
            g.push(url.to_owned());
        }
        if let Some(msg) = &self.transport_error {
            return Err(UpdateError::Transport(msg.clone()));
        }
        if let Some(r) = self.routes.get(url) {
            return Ok(r.clone());
        }
        Ok(Response {
            status: self.fallback_status,
            body: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn records_calls_and_routes() {
        let m = MockHttp::new().with_json("https://x/y", 200, r#"{"a":1}"#);
        let r = m.get("https://x/y").unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, br#"{"a":1}"#);
        assert_eq!(m.calls(), vec!["https://x/y".to_string()]);
    }

    #[test]
    fn unknown_url_falls_back_to_404() {
        let m = MockHttp::new();
        assert_eq!(m.get("https://nope").unwrap().status, 404);
    }

    #[test]
    fn transport_error_takes_precedence() {
        let m = MockHttp::new()
            .with_json("https://x", 200, "{}")
            .with_transport_error("dns 解析失败");
        let e = m.get("https://x").unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("dns"), "{msg}");
    }
}
