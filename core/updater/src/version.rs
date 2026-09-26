//! 语义化版本解析与比较。
//!
//! 单独成模块是因为**版本比较是自动更新里最容易出错、错了后果最严重的一环**：
//! 比错了要么用户永远收不到更新，要么被反复提示「有新版本」然后下载到自己
//! 已经装着的那个版本。参考实现（torto-app）用「按点切分成整数逐段比」，
//! 对 `0.1.0-rc1` 这类预发布版本会得出错误结论 —— 这里按 semver 规范实现。

use crate::UpdateError;

/// 语义化版本。`0.1.0-rc.1+build.5` → `{0,1,0, pre:"rc.1"}`（build 元数据被丢弃，
/// 因为 semver 规定它**不参与**优先级比较）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl SemVer {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            pre: None,
        }
    }
}

impl std::fmt::Display for SemVer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(p) = &self.pre {
            write!(f, "-{p}")?;
        }
        Ok(())
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering as O;
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                // 正式版 > 任何预发布版
                (None, None) => O::Equal,
                (None, Some(_)) => O::Greater,
                (Some(_), None) => O::Less,
                (Some(a), Some(b)) => cmp_pre(a, b),
            })
    }
}

/// semver §11：预发布标识符逐段比较，纯数字段按数值比，数字段 < 字母段，
/// 段数少且前缀相同的更小（`rc` < `rc.1`）。
fn cmp_pre(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering as O;
    let mut ai = a.split('.');
    let mut bi = b.split('.');
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return O::Equal,
            (None, Some(_)) => return O::Less,
            (Some(_), None) => return O::Greater,
            (Some(x), Some(y)) => {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(xn), Ok(yn)) => xn.cmp(&yn),
                    (Ok(_), Err(_)) => O::Less, // 数字标识符优先级低于字母数字
                    (Err(_), Ok(_)) => O::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if ord != O::Equal {
                    return ord;
                }
            }
        }
    }
}

/// 解析版本号。容忍 git tag 的 `v` 前缀、`+build` 元数据、`-pre` 预发布，
/// 以及只写一两段的简写（`0.1` == `0.1.0`）。
pub fn parse_version(raw: &str) -> Result<SemVer, UpdateError> {
    let s = raw.trim();
    let s = s
        .strip_prefix('v')
        .or_else(|| s.strip_prefix('V'))
        .unwrap_or(s);
    if s.is_empty() {
        return Err(UpdateError::InvalidVersion(raw.to_owned()));
    }
    // 丢掉 build 元数据（不参与比较）
    let core = s.split('+').next().unwrap_or(s);
    if core.is_empty() {
        return Err(UpdateError::InvalidVersion(raw.to_owned()));
    }
    let (nums, pre) = match core.split_once('-') {
        Some((n, p)) => (n, Some(p.to_owned())),
        None => (core, None),
    };
    if let Some(p) = &pre {
        if p.is_empty()
            || !p
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        {
            return Err(UpdateError::InvalidVersion(raw.to_owned()));
        }
    }

    let parts: Vec<&str> = nums.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return Err(UpdateError::InvalidVersion(raw.to_owned()));
    }
    let mut out = [0u64; 3];
    for (i, p) in parts.iter().enumerate() {
        // semver 禁止前导零（"01" 非法）；这里放宽为接受但按数值解析，
        // 因为 git tag 是人工打的，严格拒绝会让整个更新检查失败，得不偿失
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) {
            return Err(UpdateError::InvalidVersion(raw.to_owned()));
        }
        let v = p
            .parse::<u64>()
            .map_err(|_| UpdateError::InvalidVersion(raw.to_owned()))?;
        out[i] = v;
    }
    Ok(SemVer {
        major: out[0],
        minor: out[1],
        patch: out[2],
        pre,
    })
}

/// 便捷比较：`a` 相对 `b` 的顺序。
pub fn compare(a: &str, b: &str) -> Result<std::cmp::Ordering, UpdateError> {
    Ok(parse_version(a)?.cmp(&parse_version(b)?))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::cmp::Ordering as O;

    #[test]
    fn parses_common_forms() {
        assert_eq!(parse_version("0.1.0").unwrap().to_string(), "0.1.0");
        assert_eq!(parse_version("v0.1.0").unwrap().to_string(), "0.1.0");
        assert_eq!(parse_version("V1.2.3").unwrap().to_string(), "1.2.3");
        assert_eq!(parse_version("0.1").unwrap().to_string(), "0.1.0");
        assert_eq!(parse_version("1").unwrap().to_string(), "1.0.0");
        assert_eq!(
            parse_version("0.1.0+build.7").unwrap().to_string(),
            "0.1.0",
            "build 元数据不参与比较，应被丢弃"
        );
        assert_eq!(
            parse_version("v0.2.0-rc.1+win").unwrap().to_string(),
            "0.2.0-rc.1"
        );
    }

    #[test]
    fn rejects_garbage() {
        for bad in [
            "", "v", "abc", "1.2.3.4", "1..2", "0.1.0-", "1.2.x", "-1.0.0",
        ] {
            assert!(parse_version(bad).is_err(), "{bad:?} 应被拒绝");
        }
    }

    #[test]
    fn orders_numeric_parts() {
        assert_eq!(compare("0.1.0", "0.1.1").unwrap(), O::Less);
        assert_eq!(
            compare("0.2.0", "0.10.0").unwrap(),
            O::Less,
            "不能按字符串比"
        );
        assert_eq!(compare("1.0.0", "0.99.99").unwrap(), O::Greater);
        assert_eq!(compare("0.1.0", "0.1").unwrap(), O::Equal);
        assert_eq!(compare("v0.1.0", "0.1.0+build").unwrap(), O::Equal);
    }

    #[test]
    fn prerelease_is_older_than_release() {
        assert_eq!(compare("0.1.0-rc.1", "0.1.0").unwrap(), O::Less);
        assert_eq!(compare("0.1.0", "0.1.0-rc.1").unwrap(), O::Greater);
    }

    #[test]
    fn prerelease_identifiers_follow_semver() {
        assert_eq!(compare("0.1.0-alpha", "0.1.0-beta").unwrap(), O::Less);
        assert_eq!(compare("0.1.0-rc.1", "0.1.0-rc.2").unwrap(), O::Less);
        assert_eq!(
            compare("0.1.0-rc.10", "0.1.0-rc.2").unwrap(),
            O::Greater,
            "数字段按数值比"
        );
        assert_eq!(
            compare("0.1.0-rc", "0.1.0-rc.1").unwrap(),
            O::Less,
            "段数少且前缀相同则更小"
        );
        assert_eq!(
            compare("0.1.0-1", "0.1.0-alpha").unwrap(),
            O::Less,
            "数字段 < 字母段"
        );
    }

    #[test]
    fn update_available_predicate_matches_reference_semantics() {
        // 与 torto-app 的 `compareReleaseVersions(current, latest) < 0` 等价
        assert!(compare("0.1.0", "0.2.0").unwrap().is_lt());
        assert!(!compare("0.2.0", "0.2.0").unwrap().is_lt());
        assert!(
            !compare("0.3.0", "0.2.0").unwrap().is_lt(),
            "本地更新时不该提示降级"
        );
    }
}
