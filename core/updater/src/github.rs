//! GitHub Releases 的解析与产物挑选。
//!
//! 只用 `serde_json::Value` 手工取字段，不引入 `serde` derive：
//! GitHub 的响应字段很多且会变，我们只需要 5 个；用 `Value` 取值可以让
//! 「字段缺失」和「字段类型不对」都落到同一个明确的错误分支，
//! 而不是被反序列化框架的报错信息绕晕。

use crate::version::{parse_version, SemVer};
use crate::UpdateError;

/// 发布通道。稳定通道会跳过 GitHub 上标记为 prerelease 的版本，
/// 否则用户打了 `v0.2.0-rc.1` 的 tag 就会推送给所有人。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channel {
    #[default]
    Stable,
    Beta,
}

/// 目标平台。产物命名规则见 `.github/workflows/release.yml` 与
/// `platforms/windows/installer/package.ps1`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    WindowsX64,
    WindowsX86,
    AndroidArm64,
    AndroidArm32,
    AndroidX64,
}

impl Platform {
    /// 产物文件名后缀。用后缀匹配而不是全名匹配，这样版本号变化不需要改代码。
    ///
    /// Windows 发的是 **Inno Setup 安装包**而不是 zip：
    /// 输入法要写 HKLM 的 CTF 注册表键、要把 8.9MB 词库放进 Program Files、
    /// 还要处理「DLL 正被所有进程占用」—— 这些让用户手动解压+跑脚本是不负责任的。
    /// 自动更新因此变成「下载 setup.exe → 校验 sha256 → `/VERYSILENT` 静默升级」。
    pub fn asset_suffix(self) -> &'static str {
        match self {
            Self::WindowsX64 => "-windows-x64-setup.exe",
            Self::WindowsX86 => "-windows-x86-setup.exe",
            Self::AndroidArm64 => "-android-arm64.apk",
            Self::AndroidArm32 => "-android-arm32.apk",
            Self::AndroidX64 => "-android-x64.apk",
        }
    }

    pub fn current() -> Option<Self> {
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        return Some(Self::WindowsX64);
        #[cfg(all(target_os = "windows", target_arch = "x86"))]
        return Some(Self::WindowsX86);
        #[cfg(all(target_os = "android", target_arch = "aarch64"))]
        return Some(Self::AndroidArm64);
        #[cfg(all(target_os = "android", target_arch = "arm"))]
        return Some(Self::AndroidArm32);
        #[cfg(all(target_os = "android", target_arch = "x86_64"))]
        return Some(Self::AndroidX64);
        #[cfg(not(any(
            all(
                target_os = "windows",
                any(target_arch = "x86_64", target_arch = "x86")
            ),
            all(
                target_os = "android",
                any(target_arch = "aarch64", target_arch = "arm", target_arch = "x86_64")
            ),
        )))]
        return None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    /// `browser_download_url`，直接可下载（不需要 API 鉴权）
    pub url: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: SemVer,
    pub html_url: String,
    pub name: Option<String>,
    pub published_at: Option<String>,
    pub prerelease: bool,
    pub assets: Vec<ReleaseAsset>,
}

impl Release {
    /// 该平台的主产物（zip/apk）。
    pub fn asset(&self, p: Platform) -> Option<&ReleaseAsset> {
        let suffix = p.asset_suffix();
        self.assets.iter().find(|a| a.name.ends_with(suffix))
    }

    /// 主产物对应的 sha256 旁文件。
    ///
    /// 校验文件必须和产物**成对存在**才允许安装：只下产物不校验，
    /// 等于把「下载被中间人替换」这条攻击路径完全打开。
    pub fn checksum_asset(&self, p: Platform) -> Option<&ReleaseAsset> {
        let suffix = format!("{}.sha256", p.asset_suffix());
        self.assets.iter().find(|a| a.name.ends_with(&suffix))
    }
}

fn str_field<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

/// 解析 `GET /repos/{owner}/{repo}/releases/latest` 的响应体。
pub fn parse_release_json(body: &[u8]) -> Result<Release, UpdateError> {
    let v: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| UpdateError::Malformed(format!("不是合法 JSON: {e}")))?;
    if v.get("message").and_then(|m| m.as_str()).is_some() && v.get("tag_name").is_none() {
        // GitHub 出错时返回 {"message": "..."}，把它当成 malformed 会把真实原因吞掉
        let msg = str_field(&v, "message").unwrap_or("unknown");
        return Err(UpdateError::Malformed(format!("GitHub 返回错误: {msg}")));
    }

    let tag = str_field(&v, "tag_name")
        .ok_or_else(|| UpdateError::Malformed("缺少 tag_name".into()))?
        .to_owned();
    let html_url = str_field(&v, "html_url")
        .ok_or_else(|| UpdateError::Malformed("缺少 html_url".into()))?
        .to_owned();
    if !html_url.starts_with("https://") {
        return Err(UpdateError::Malformed(format!(
            "release URL 不是 https: {html_url}"
        )));
    }
    let version = parse_version(&tag)
        .map_err(|_| UpdateError::Malformed(format!("tag 不是合法版本号: {tag}")))?;

    let assets = v
        .get("assets")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    Some(ReleaseAsset {
                        name: str_field(a, "name")?.to_owned(),
                        url: str_field(a, "browser_download_url")?.to_owned(),
                        size: a.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Release {
        tag,
        version,
        html_url,
        name: str_field(&v, "name").map(str::to_owned),
        published_at: str_field(&v, "published_at").map(str::to_owned),
        prerelease: v
            .get("prerelease")
            .and_then(|p| p.as_bool())
            .unwrap_or(false),
        assets,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn sample_release() -> &'static str {
        r#"{
          "tag_name": "v0.2.0",
          "name": "retype 0.2.0",
          "html_url": "https://github.com/acme/retype/releases/tag/v0.2.0",
          "published_at": "2026-09-26T10:00:00Z",
          "prerelease": false,
          "assets": [
            {"name": "retype-0.2.0-windows-x64-setup.exe",
             "browser_download_url": "https://example.invalid/retype-0.2.0-windows-x64-setup.exe",
             "size": 9000000},
            {"name": "retype-0.2.0-windows-x64-setup.exe.sha256",
             "browser_download_url": "https://example.invalid/retype-0.2.0-windows-x64-setup.exe.sha256",
             "size": 120},
            {"name": "retype-0.2.0-windows-x86-setup.exe",
             "browser_download_url": "https://example.invalid/x86.exe", "size": 8000000}
          ]
        }"#
    }

    #[test]
    fn parses_a_release() {
        let r = parse_release_json(sample_release().as_bytes()).unwrap();
        assert_eq!(r.tag, "v0.2.0");
        assert_eq!(r.version.to_string(), "0.2.0");
        assert!(!r.prerelease);
        assert_eq!(r.assets.len(), 3);
        assert_eq!(r.published_at.as_deref(), Some("2026-09-26T10:00:00Z"));
    }

    #[test]
    fn picks_assets_by_platform() {
        let r = parse_release_json(sample_release().as_bytes()).unwrap();
        let a = r.asset(Platform::WindowsX64).unwrap();
        assert_eq!(a.name, "retype-0.2.0-windows-x64-setup.exe");
        assert_eq!(a.size, 9000000);
        assert_eq!(
            r.asset(Platform::WindowsX86).unwrap().name,
            "retype-0.2.0-windows-x86-setup.exe"
        );
        assert!(
            r.asset(Platform::AndroidArm64).is_none(),
            "没发布 android 产物就该是 None"
        );
    }

    #[test]
    fn checksum_asset_is_paired_with_the_artifact() {
        let r = parse_release_json(sample_release().as_bytes()).unwrap();
        let c = r.checksum_asset(Platform::WindowsX64).unwrap();
        assert!(c.name.ends_with("-setup.exe.sha256"));
        assert!(r.checksum_asset(Platform::WindowsX86).is_none());
    }

    #[test]
    fn github_error_body_is_reported_not_swallowed() {
        let e = parse_release_json(br#"{"message":"API rate limit exceeded"}"#).unwrap_err();
        let msg = e.to_string();
        assert!(
            msg.contains("rate limit"),
            "应保留 GitHub 的原始错误: {msg}"
        );
    }

    #[test]
    fn missing_or_bad_fields_are_rejected() {
        assert!(parse_release_json(b"not json").is_err());
        assert!(parse_release_json(br#"{"html_url":"https://x"}"#).is_err());
        assert!(parse_release_json(br#"{"tag_name":"v0.1.0"}"#).is_err());
        // 非 https 的下载地址必须拒绝：否则更新通道可以被降级成明文
        assert!(
            parse_release_json(br#"{"tag_name":"v0.1.0","html_url":"http://evil.invalid/r"}"#)
                .is_err()
        );
        assert!(parse_release_json(
            br#"{"tag_name":"not-a-version","html_url":"https://x.invalid"}"#
        )
        .is_err());
    }

    #[test]
    fn missing_assets_field_means_no_assets() {
        let r =
            parse_release_json(br#"{"tag_name":"v0.1.0","html_url":"https://x.invalid"}"#).unwrap();
        assert!(r.assets.is_empty());
        assert!(r.asset(Platform::WindowsX64).is_none());
    }

    #[test]
    fn platform_suffixes_are_distinct() {
        use std::collections::HashSet;
        let all = [
            Platform::WindowsX64,
            Platform::WindowsX86,
            Platform::AndroidArm64,
            Platform::AndroidArm32,
            Platform::AndroidX64,
        ];
        let set: HashSet<&str> = all.iter().map(|p| p.asset_suffix()).collect();
        assert_eq!(set.len(), all.len(), "后缀必须互不相同，否则会挑错产物");
    }
}
