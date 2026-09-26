//! 上下文采集接口与隐私闸门。
//!
//! **采集实现属于平台层**（Windows: TSF `ITfContext` + 前台进程；Android: `EditorInfo`），
//! 这里只定义契约和策略。两条铁律来自 ARCHITECTURE.md：
//!
//! 1. 采集失败 = 空上下文，不是错误（P2）。很多应用会拒绝读取光标周围文本。
//! 2. 隐私等级由**采集方盖章**，云端出网前必须再过一次
//!    [`ContextSnapshot::sanitized_for_cloud`]，双重保险。
#![forbid(unsafe_code)]

use retype_types::{AppInfo, ContextSnapshot, FieldInfo, FieldKind, PrivacyLevel};

/// 平台层实现的上下文采集器。
///
/// 实现必须：非阻塞（或自带超时）、失败返回 `None`、绝不 panic。
pub trait ContextCollector: Send + Sync {
    fn collect(&self) -> Option<ContextSnapshot>;
}

/// 永远采集不到东西的实现。用于测试降级路径，也是采集模块初始化失败时的兜底。
#[derive(Debug, Clone, Copy, Default)]
pub struct NullCollector;

impl ContextCollector for NullCollector {
    fn collect(&self) -> Option<ContextSnapshot> {
        None
    }
}

/// 固定的快照，测试用。
#[derive(Debug, Clone)]
pub struct FixedCollector {
    pub snapshot: Option<ContextSnapshot>,
}

impl ContextCollector for FixedCollector {
    fn collect(&self) -> Option<ContextSnapshot> {
        self.snapshot.clone()
    }
}

/// 隐私策略：进程黑/白名单 + 输入框类型规则。
///
/// 对应 test.md 第三节里那句「输入框是否允许读取上下文」——
/// 这不是可选项，是产品能不能被信任的前提。
#[derive(Debug, Clone)]
pub struct PrivacyPolicy {
    /// 这些进程一律 `None`（不采集）。默认包含常见密码管理器。
    pub denylist: Vec<String>,
    /// `Some` 表示只有名单内的进程才允许 `Cloud` 级别，其余最多 `Local`。
    pub cloud_allowlist: Option<Vec<String>>,
    /// 默认等级
    pub default_level: PrivacyLevel,
}

impl Default for PrivacyPolicy {
    fn default() -> Self {
        Self {
            denylist: DEFAULT_DENYLIST.iter().map(|s| (*s).to_owned()).collect(),
            cloud_allowlist: None,
            default_level: PrivacyLevel::Local,
        }
    }
}

/// 默认黑名单：密码管理器与银行类客户端。
/// 匹配方式是「进程名/包名包含该子串（小写）」，宁可误杀。
pub static DEFAULT_DENYLIST: &[&str] = &[
    "keepass",
    "1password",
    "onepassword",
    "bitwarden",
    "lastpass",
    "enpass",
    "dashlane",
    "passwordsafe",
    "alilang", // 阿里郎/内网安全客户端
];

impl PrivacyPolicy {
    pub fn evaluate(&self, app: &AppInfo, field: &FieldInfo) -> PrivacyLevel {
        // 密码框：无条件不采集，任何策略都不能覆盖
        if field.kind == FieldKind::Password {
            return PrivacyLevel::None;
        }
        let package = app.package.to_ascii_lowercase();
        if !package.is_empty() && self.denylist.iter().any(|d| package.contains(d)) {
            return PrivacyLevel::None;
        }
        if self.default_level == PrivacyLevel::Cloud {
            // 只有显式允许的应用才能把上下文送出网
            let allowed = match &self.cloud_allowlist {
                Some(list) => list
                    .iter()
                    .any(|a| package.contains(&a.to_ascii_lowercase())),
                None => true,
            };
            return if allowed {
                PrivacyLevel::Cloud
            } else {
                PrivacyLevel::Local
            };
        }
        self.default_level
    }

    /// 给一份「还没盖章」的快照打上隐私等级，并按等级裁剪内容。
    pub fn stamp(&self, mut snap: ContextSnapshot) -> ContextSnapshot {
        let level = self.evaluate(&snap.app, &snap.field);
        snap.privacy = level;
        match level {
            PrivacyLevel::None => ContextSnapshot {
                app: snap.app,
                field: snap.field,
                privacy: PrivacyLevel::None,
                ..Default::default()
            },
            _ => snap,
        }
    }
}

/// 光标前文本的默认取样长度（字符数）。
///
/// 取太多会显著增加云端 token 成本与隐私风险，取太少又失去语境。
/// test.md 第三节的例子（「人工智能大模型」+「推理」）说明 32~64 字已足够。
pub const DEFAULT_BEFORE_CHARS: usize = 64;
pub const DEFAULT_AFTER_CHARS: usize = 16;

/// 把宿主给出的原始文本裁剪成快照该带的长度。
pub fn trim_context(before: &str, after: &str) -> (String, String) {
    let b_total = before.chars().count();
    let b: String = before
        .chars()
        .skip(b_total.saturating_sub(DEFAULT_BEFORE_CHARS))
        .collect();
    let a: String = after.chars().take(DEFAULT_AFTER_CHARS).collect();
    (b, a)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn field(kind: FieldKind) -> FieldInfo {
        FieldInfo {
            kind,
            context_readable: true,
            max_length: None,
        }
    }

    #[test]
    fn password_field_is_never_collected() {
        let p = PrivacyPolicy {
            default_level: PrivacyLevel::Cloud,
            ..Default::default()
        };
        let app = AppInfo::new("chrome.exe");
        assert_eq!(
            p.evaluate(&app, &field(FieldKind::Password)),
            PrivacyLevel::None
        );
    }

    #[test]
    fn denylisted_process_is_never_collected() {
        let p = PrivacyPolicy {
            default_level: PrivacyLevel::Cloud,
            ..Default::default()
        };
        assert_eq!(
            p.evaluate(&AppInfo::new("KeePassXC.exe"), &field(FieldKind::Unknown)),
            PrivacyLevel::None
        );
        assert_eq!(
            p.evaluate(
                &AppInfo::new("com.agilebits.onepassword"),
                &field(FieldKind::Chat)
            ),
            PrivacyLevel::None
        );
    }

    #[test]
    fn cloud_requires_allowlist_when_configured() {
        let p = PrivacyPolicy {
            default_level: PrivacyLevel::Cloud,
            cloud_allowlist: Some(vec!["wechat.exe".into()]),
            ..Default::default()
        };
        assert_eq!(
            p.evaluate(&AppInfo::new("WeChat.exe"), &field(FieldKind::Chat)),
            PrivacyLevel::Cloud
        );
        assert_eq!(
            p.evaluate(&AppInfo::new("notepad.exe"), &field(FieldKind::Document)),
            PrivacyLevel::Local
        );
    }

    #[test]
    fn stamp_clears_text_for_none_level() {
        let p = PrivacyPolicy::default();
        let snap = ContextSnapshot {
            app: AppInfo::new("keepass.exe"),
            field: field(FieldKind::Unknown),
            text_before: "主密码是".into(),
            privacy: PrivacyLevel::Cloud, // 采集方盖错了章，策略要能纠正
            ..Default::default()
        };
        let out = p.stamp(snap);
        assert_eq!(out.privacy, PrivacyLevel::None);
        assert!(out.text_before.is_empty());
    }

    #[test]
    fn trim_keeps_tail_of_before_and_head_of_after() {
        let long: String = (0..200)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let (b, a) = trim_context(&long, &long);
        assert_eq!(b.chars().count(), DEFAULT_BEFORE_CHARS);
        assert_eq!(a.chars().count(), DEFAULT_AFTER_CHARS);
        assert!(long.ends_with(&b), "应保留靠近光标的那一段");
    }

    #[test]
    fn null_collector_degrades_to_none() {
        assert!(NullCollector.collect().is_none());
    }
}
