//! 自动更新核心。
//!
//! ## 为什么这是一个独立 crate，而且不做网络 IO
//!
//! 参考实现（torto-app `lib/app/update/app_update_service.dart`）把「查 GitHub
//! releases/latest → 比对版本 → 给出 release 页」放在一起，HTTP client 可注入。
//! retype 沿用同样的契约，但把**网络实现**推到平台层：
//!
//! - `core/updater` 只有纯逻辑 + [`HttpFetcher`] trait，可以完全离线单测；
//! - `platforms/windows/updater`（`retype-updater.exe`）用 `ureq` 提供真实实现；
//! - Android 端将来用同一份逻辑，只换一个 fetcher。
//!
//! **更新检查绝不能放进 TIP DLL**：它被注入到每一个宿主进程，
//! 在里面发 HTTP 请求等于让 Chrome、Word 全都替我们打网络流量，
//! 而且直接违反 P1（输入主线程不得阻塞）。所以更新是一个**独立的 exe**，
//! 由设置界面或计划任务触发。
//!
//! ## 更新流程（M0 实现到「校验通过」为止）
//!
//! ```text
//! check ──► 比对版本 ──► 有新版？
//!                          │否 → 静默结束（不打扰用户）
//!                          │是
//!                          ▼
//!              下载 <产物>.sha256  ──► 下载产物 ──► verify_sha256
//!                                                      │失败 → 删除，报错，绝不安装
//!                                                      │通过
//!                                                      ▼
//!                                          落盘到暂存区，等待 M5 的安装器接管
//! ```
//!
//! 「替换正在使用的 DLL」这一步刻意**没有**实现：TIP 被所有进程加载着，
//! 直接覆盖会失败或造成半更新状态。真正的原子替换（改名 + 重启后清理，
//! 或走 MSI）是 M5 安装器的工作，见 `docs/auto-update.md`。

pub mod github;
pub mod mock;
pub mod verify;
pub mod version;

pub use github::{parse_release_json, Channel, Platform, Release, ReleaseAsset};
pub use mock::MockHttp;
pub use verify::{expected_for, parse_sha256sum, sha256_hex, verify_sha256};
pub use version::{compare, parse_version, SemVer};

use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("版本号非法: {0:?}")]
    InvalidVersion(String),
    #[error("仓库未配置（需要 owner/name 形式）")]
    RepoNotConfigured,
    #[error("HTTP {status}: {url}")]
    Http { status: u16, url: String },
    #[error("网络错误: {0}")]
    Transport(String),
    #[error("响应无法解析: {0}")]
    Malformed(String),
    #[error("校验失败: 期望 {expected}，实际 {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("缺少校验文件，拒绝安装（更新通道必须可验证）")]
    MissingChecksum,
    #[error("IO 失败: {0}")]
    Io(String),
}

impl From<std::io::Error> for UpdateError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// HTTP 响应。只保留状态码和 body —— 更新检查用不到别的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// 传输层抽象。
///
/// 实现方**必须自带超时**（建议 15s，与参考实现一致）：
/// 更新检查通常发生在设置界面打开时，卡住会让整个界面失去响应。
pub trait HttpFetcher: Send + Sync {
    fn get(&self, url: &str) -> Result<Response, UpdateError>;
}

/// 一次更新检查的结论。
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateStatus {
    pub current: SemVer,
    /// `None` 表示「仓库还没有任何 release」或「稳定通道下最新的是预发布版」。
    /// 这两种情况都**不是错误**：一个新项目在没有发布之前，检查更新应该安静地返回。
    pub latest: Option<Release>,
    pub update_available: bool,
    /// 给人看的发布页（参考实现的做法：把用户带到 release 页，而不是偷偷装）
    pub release_page: Option<String>,
    /// 已按平台挑好的主产物
    pub asset: Option<ReleaseAsset>,
    /// 主产物的 sha256 旁文件。**缺失时不许安装**（见 `MissingChecksum`）
    pub checksum_asset: Option<ReleaseAsset>,
}

impl UpdateStatus {
    pub fn up_to_date(current: SemVer) -> Self {
        Self {
            current,
            latest: None,
            update_available: false,
            release_page: None,
            asset: None,
            checksum_asset: None,
        }
    }

    /// 是否可以安全下载：有产物、有校验文件、且确实比当前版本新。
    pub fn is_installable(&self) -> bool {
        self.update_available && self.asset.is_some() && self.checksum_asset.is_some()
    }
}

pub struct UpdateChecker {
    repo: String,
    http: Arc<dyn HttpFetcher>,
    channel: Channel,
}

impl std::fmt::Debug for UpdateChecker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateChecker")
            .field("repo", &self.repo)
            .field("channel", &self.channel)
            .finish_non_exhaustive()
    }
}

impl UpdateChecker {
    /// `repo` 形如 `owner/name`。
    pub fn new(repo: impl Into<String>, http: Arc<dyn HttpFetcher>) -> Self {
        Self {
            repo: repo.into(),
            http,
            channel: Channel::default(),
        }
    }

    pub fn with_channel(mut self, channel: Channel) -> Self {
        self.channel = channel;
        self
    }

    pub fn repo(&self) -> &str {
        &self.repo
    }

    pub fn api_url(&self) -> String {
        format!(
            "https://api.github.com/repos/{}/releases/latest",
            self.repo.trim_matches('/')
        )
    }

    /// 检查是否有新版本。
    ///
    /// 404 → `Ok`（还没发布过，不是错误）；其他非 2xx → `Err`；
    /// 传输失败 → `Err`。调用方应该把 `Err` 当成「这次查不到」而不是「出事了」，
    /// 绝不让更新检查失败影响主功能。
    pub fn check(&self, current: &str, platform: Platform) -> Result<UpdateStatus, UpdateError> {
        if !is_valid_repo(&self.repo) {
            return Err(UpdateError::RepoNotConfigured);
        }
        let current = parse_version(current)?;
        let url = self.api_url();
        let resp = self.http.get(&url)?;

        // GitHub 对「还没有任何 release」的仓库返回 404，这是正常状态
        if resp.status == 404 {
            return Ok(UpdateStatus::up_to_date(current));
        }
        if !(200..300).contains(&resp.status) {
            return Err(UpdateError::Http {
                status: resp.status,
                url,
            });
        }

        let release = parse_release_json(&resp.body)?;
        // GitHub 的 /releases/latest 本身就不会返回 prerelease，
        // 这里再挡一道：将来若改用 /releases 列表也不会漏
        if release.prerelease && self.channel == Channel::Stable {
            return Ok(UpdateStatus::up_to_date(current));
        }

        let update_available = release.version > current;
        Ok(UpdateStatus {
            release_page: Some(release.html_url.clone()),
            asset: release.asset(platform).cloned(),
            checksum_asset: release.checksum_asset(platform).cloned(),
            current,
            latest: Some(release),
            update_available,
        })
    }
}

/// `owner/name`，两段都非空且不含空白与多余斜杠。
fn is_valid_repo(repo: &str) -> bool {
    let r = repo.trim();
    let mut parts = r.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let ok = |s: &str| {
        !s.is_empty() && !s.chars().any(|c| c.is_whitespace()) && !s.contains("..") && s != "."
    };
    ok(owner) && ok(name)
}

/// 编译期烘进来的仓库（CI 用 `RETYPE_GITHUB_REPO` 环境变量注入 `github.repository`）。
///
/// 这样发布出去的 exe 天然知道自己的仓库，而开发构建可以退回运行时参数，
/// 不需要在源码里硬编码一个可能改名/迁移的地址。
pub fn baked_repo() -> Option<&'static str> {
    match option_env!("RETYPE_GITHUB_REPO") {
        Some(r) if is_valid_repo(r) => Some(r),
        _ => None,
    }
}

/// 运行时仓库来源：`--repo` 参数 > `RETYPE_GITHUB_REPO` 环境变量 > 编译期烘入值。
pub fn resolve_repo(cli_value: Option<&str>) -> Option<String> {
    if let Some(v) = cli_value {
        let v = v.trim();
        if is_valid_repo(v) {
            return Some(v.to_owned());
        }
    }
    if let Ok(v) = std::env::var("RETYPE_GITHUB_REPO") {
        if is_valid_repo(&v) {
            return Some(v);
        }
    }
    baked_repo().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    const REPO: &str = "acme/retype";
    const URL: &str = "https://api.github.com/repos/acme/retype/releases/latest";

    fn release_json(tag: &str, with_assets: bool) -> String {
        let assets = if with_assets {
            r#","assets":[
                {"name":"retype-0.2.0-windows-x64.zip","browser_download_url":"https://dl.invalid/a.zip","size":9000},
                {"name":"retype-0.2.0-windows-x64.zip.sha256","browser_download_url":"https://dl.invalid/a.zip.sha256","size":120}
            ]"#
        } else {
            r#","assets":[]"#
        };
        format!(
            r#"{{"tag_name":"{tag}","html_url":"https://github.com/acme/retype/releases/tag/{tag}","prerelease":false{assets}}}"#
        )
    }

    fn checker(mock: MockHttp) -> UpdateChecker {
        UpdateChecker::new(REPO, Arc::new(mock))
    }

    #[test]
    fn detects_an_available_update() {
        let m = MockHttp::new().with_json(URL, 200, &release_json("v0.2.0", true));
        let s = checker(m).check("0.1.0", Platform::WindowsX64).unwrap();
        assert!(s.update_available);
        assert!(s.is_installable());
        assert_eq!(s.latest.unwrap().version.to_string(), "0.2.0");
        assert_eq!(
            s.release_page.as_deref(),
            Some("https://github.com/acme/retype/releases/tag/v0.2.0")
        );
        assert_eq!(s.asset.unwrap().name, "retype-0.2.0-windows-x64.zip");
        assert!(s.checksum_asset.is_some());
    }

    #[test]
    fn up_to_date_reports_no_update() {
        let m = MockHttp::new().with_json(URL, 200, &release_json("v0.1.0", true));
        let s = checker(m).check("0.1.0", Platform::WindowsX64).unwrap();
        assert!(!s.update_available);
        assert!(s.latest.is_some(), "应该仍然报告最新版本号，只是不需要更新");
        assert!(!s.is_installable());
    }

    #[test]
    fn local_ahead_of_remote_does_not_offer_a_downgrade() {
        let m = MockHttp::new().with_json(URL, 200, &release_json("v0.1.0", true));
        let s = checker(m).check("0.3.0", Platform::WindowsX64).unwrap();
        assert!(!s.update_available, "本地更新时不该提示「有新版」");
    }

    #[test]
    fn no_release_yet_is_not_an_error() {
        // 新仓库在打第一个 tag 之前，检查更新必须安静地成功
        let m = MockHttp::new(); // 默认 fallback 404
        let s = checker(m).check("0.1.0", Platform::WindowsX64).unwrap();
        assert!(!s.update_available);
        assert!(s.latest.is_none());
    }

    #[test]
    fn rate_limit_is_an_error_not_a_silent_no_update() {
        let m = MockHttp::new().with_status(URL, 403);
        let e = checker(m).check("0.1.0", Platform::WindowsX64).unwrap_err();
        assert!(matches!(e, UpdateError::Http { status: 403, .. }), "{e}");
    }

    #[test]
    fn transport_failure_propagates() {
        let m = MockHttp::new().with_transport_error("断网");
        let e = checker(m).check("0.1.0", Platform::WindowsX64).unwrap_err();
        assert!(matches!(e, UpdateError::Transport(_)), "{e}");
    }

    #[test]
    fn broken_json_is_rejected() {
        let m = MockHttp::new().with_json(URL, 200, "<html>not json</html>");
        assert!(matches!(
            checker(m).check("0.1.0", Platform::WindowsX64).unwrap_err(),
            UpdateError::Malformed(_)
        ));
    }

    #[test]
    fn bad_current_version_is_rejected_before_any_request() {
        let m = Arc::new(MockHttp::new().with_json(URL, 200, &release_json("v0.2.0", false)));
        let c = UpdateChecker::new(REPO, Arc::clone(&m) as Arc<dyn HttpFetcher>);
        assert!(matches!(
            c.check("not-a-version", Platform::WindowsX64).unwrap_err(),
            UpdateError::InvalidVersion(_)
        ));
        assert_eq!(m.call_count(), 0, "版本号非法时不该发请求");
    }

    #[test]
    fn prerelease_is_hidden_on_the_stable_channel() {
        let body = r#"{"tag_name":"v0.2.0-rc.1","html_url":"https://github.com/acme/retype/releases/tag/v0.2.0-rc.1","prerelease":true,"assets":[]}"#;
        let m = MockHttp::new().with_json(URL, 200, body);
        let s = checker(m).check("0.1.0", Platform::WindowsX64).unwrap();
        assert!(!s.update_available, "稳定通道不该推送 rc 版");

        let m = MockHttp::new().with_json(URL, 200, body);
        let s = UpdateChecker::new(REPO, Arc::new(m))
            .with_channel(Channel::Beta)
            .check("0.1.0", Platform::WindowsX64)
            .unwrap();
        assert!(s.update_available, "beta 通道应该能看到 rc 版");
    }

    #[test]
    fn missing_assets_means_not_installable_but_still_notifies() {
        let m = MockHttp::new().with_json(URL, 200, &release_json("v0.2.0", false));
        let s = checker(m).check("0.1.0", Platform::WindowsX64).unwrap();
        assert!(s.update_available);
        assert!(!s.is_installable(), "没有产物就只能引导用户去 release 页");
        assert!(s.release_page.is_some());
    }

    #[test]
    fn missing_checksum_blocks_install() {
        // 只发了 zip 没发 .sha256：必须判定为不可安装，否则更新通道无法验证
        let body = r#"{"tag_name":"v0.2.0","html_url":"https://github.com/acme/retype/releases/tag/v0.2.0","prerelease":false,"assets":[
            {"name":"retype-0.2.0-windows-x64.zip","browser_download_url":"https://dl.invalid/a.zip","size":9000}]}"#;
        let m = MockHttp::new().with_json(URL, 200, body);
        let s = checker(m).check("0.1.0", Platform::WindowsX64).unwrap();
        assert!(s.update_available);
        assert!(s.asset.is_some());
        assert!(!s.is_installable(), "缺 sha256 旁文件时不许安装");
    }

    #[test]
    fn unconfigured_repo_is_rejected() {
        for bad in [
            "",
            "/",
            "acme",
            "acme/",
            "/retype",
            "a/b/c",
            "ac me/retype",
            "acme/../x",
        ] {
            let m = MockHttp::new().with_json(
                &format!("https://api.github.com/repos/{bad}/releases/latest"),
                200,
                &release_json("v0.2.0", true),
            );
            let e = UpdateChecker::new(bad, Arc::new(m))
                .check("0.1.0", Platform::WindowsX64)
                .unwrap_err();
            assert!(matches!(e, UpdateError::RepoNotConfigured), "{bad:?} → {e}");
        }
    }

    #[test]
    fn api_url_is_well_formed() {
        assert_eq!(checker(MockHttp::new()).api_url(), URL);
        assert_eq!(
            UpdateChecker::new("acme/retype/", Arc::new(MockHttp::new())).api_url(),
            URL
        );
    }

    #[test]
    fn resolve_repo_accepts_explicit_value() {
        assert_eq!(resolve_repo(Some("o/n")).as_deref(), Some("o/n"));
        assert_eq!(
            resolve_repo(Some("  acme/retype  ")).as_deref(),
            Some("acme/retype")
        );
    }

    #[test]
    fn resolve_repo_rejects_garbage() {
        // 显式值非法时不会「凑合用」，而是继续往环境变量/编译期值找；
        // 这个测试不碰进程环境（并行测试下改 env 会互相干扰），只断言不会返回垃圾值
        for bad in ["", " ", "acme", "a/b/c", "ac me/x"] {
            let got = resolve_repo(Some(bad));
            assert!(
                got.as_deref().map(is_valid_repo).unwrap_or(true),
                "{bad:?} 不该被当成合法仓库: {got:?}"
            );
        }
    }

    #[test]
    fn baked_repo_is_absent_in_dev_builds() {
        // CI 会通过 RETYPE_GITHUB_REPO 烘入真实仓库；
        // 本地开发构建没有这个环境变量，必须返回 None 而不是一个占位地址
        if std::env::var_os("RETYPE_GITHUB_REPO").is_none() {
            assert!(baked_repo().is_none());
        }
    }
}
