//! 上下文快照与隐私闸门（ARCHITECTURE.md §4，对应 test.md 图 3）。

/// 隐私等级。**由采集方判定并盖章**，云端模块在发请求前必须校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum PrivacyLevel {
    /// 密码框、用户显式禁止的进程：不采集，且上下文为空
    None,
    /// 只允许参与本地打分，绝不出网。默认最保守值。
    #[default]
    Local,
    /// 允许随请求发给云端
    Cloud,
}

/// 输入框类型。用于场景化排序（test.md 第三节：同一发音在不同场景代表不同文字）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FieldKind {
    #[default]
    Unknown,
    Chat,
    Search,
    Code,
    Document,
    Terminal,
    Password,
    Url,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct AppInfo {
    /// Windows: 进程名（chrome.exe）；Android: 包名
    pub package: String,
    pub window_title: String,
    /// 控件类型，若平台能拿到
    pub control_kind: Option<String>,
}

impl AppInfo {
    pub fn unknown() -> Self {
        Self::default()
    }

    pub fn new(package: impl Into<String>) -> Self {
        Self {
            package: package.into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FieldInfo {
    pub kind: FieldKind,
    /// 宿主是否允许读取光标周围文本
    pub context_readable: bool,
    pub max_length: Option<u32>,
}

/// 一次输入所参考的全部上下文。
///
/// 关键约束：**采集失败 = 空快照，不是错误**（ARCHITECTURE.md §7）。
/// 很多应用（部分游戏、UWP 沙箱、密码框）会拒绝读取，此时必须静默降级。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextSnapshot {
    pub app: AppInfo,
    pub field: FieldInfo,
    /// 光标前文本，最多 N 字（默认 64）
    pub text_before: String,
    /// 光标后文本，最多 M 字（默认 16）
    pub text_after: String,
    /// 当前选中文字（决定上屏是「插入」还是「替换」）
    pub selection: Option<String>,
    pub privacy: PrivacyLevel,
}

impl ContextSnapshot {
    pub fn empty() -> Self {
        Self {
            privacy: PrivacyLevel::None,
            ..Default::default()
        }
    }

    /// 是否携带可用于打分的文本。
    pub fn has_text(&self) -> bool {
        !self.text_before.is_empty() || self.selection.is_some()
    }

    /// 取出光标前最多 `n` 个**字符**（不是字节，中文按字算）。
    pub fn tail_before(&self, n: usize) -> String {
        let total = self.text_before.chars().count();
        self.text_before
            .chars()
            .skip(total.saturating_sub(n))
            .collect()
    }

    /// 生成一份「可以出网」的副本。
    ///
    /// 这是隐私闸门的落点：`privacy != Cloud` 时把上下文字段清空，
    /// 使得「上下文意外泄漏」在数据层面不可能发生，而不是依赖调用方自觉。
    pub fn sanitized_for_cloud(&self) -> Self {
        if self.privacy == PrivacyLevel::Cloud {
            self.clone()
        } else {
            Self {
                app: AppInfo::default(),
                field: self.field,
                text_before: String::new(),
                text_after: String::new(),
                selection: None,
                privacy: PrivacyLevel::None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn cloud_context_passes_through() {
        let snap = ContextSnapshot {
            text_before: "人工智能大模型".into(),
            privacy: PrivacyLevel::Cloud,
            ..Default::default()
        };
        assert_eq!(snap.sanitized_for_cloud().text_before, "人工智能大模型");
    }

    #[test]
    fn local_context_is_stripped_before_leaving_process() {
        let snap = ContextSnapshot {
            app: AppInfo::new("wechat.exe"),
            text_before: "我的密码是".into(),
            selection: Some("123456".into()),
            privacy: PrivacyLevel::Local,
            ..Default::default()
        };
        let out = snap.sanitized_for_cloud();
        assert!(out.text_before.is_empty());
        assert!(out.selection.is_none());
        assert!(out.app.package.is_empty());
        assert_eq!(out.privacy, PrivacyLevel::None);
    }

    #[test]
    fn tail_before_counts_chars_not_bytes() {
        let snap = ContextSnapshot {
            text_before: "人工智能大模型".into(),
            ..Default::default()
        };
        assert_eq!(snap.tail_before(3), "大模型");
        assert_eq!(snap.tail_before(100), "人工智能大模型");
    }

    #[test]
    fn empty_snapshot_is_not_none_privacy_by_default() {
        assert_eq!(ContextSnapshot::default().privacy, PrivacyLevel::Local);
        assert_eq!(ContextSnapshot::empty().privacy, PrivacyLevel::None);
    }
}
