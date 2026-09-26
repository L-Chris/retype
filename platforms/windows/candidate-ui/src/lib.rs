//! 候选窗呈现接口。
//!
//! 内核只产出 [`RenderState`]，「怎么画」完全是平台的事。把这一层单独抽出来有两个原因：
//!
//! 1. **渲染失败必须能降级**（ARCHITECTURE.md §7）：分层窗口创建失败时退回 TSF 原生
//!    `ITfCandidateListUIElement`，功能还在，只是样式变丑。
//! 2. **`retype-diag` 需要一个文本呈现器**，这样在终端里就能验证内核，
//!    不必每次都注册 TSF（docs/windows-tsf.md 的「开发回路」）。
//!
//! M1 用 TSF 原生候选窗，M2 换 Win32 + Direct2D 自绘（独立 UI 线程）。
#![forbid(unsafe_code)]

use retype_types::{CandidateSource, RenderState, StatusFlags};

/// 候选窗锚点（屏幕坐标，像素）。
///
/// 由平台层从光标位置算出：TSF 侧优先 `ITfRange::GetBoundingClientRect`，
/// 拿不到再退回 `GetCaretPos`。拿不到任何位置时传 `None`，
/// 呈现器应自己挑一个不遮挡输入的位置（通常是屏幕左下角）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScreenPoint {
    pub x: i32,
    pub y: i32,
}

pub trait CandidatePresenter: Send {
    /// 显示/更新候选窗。必须能处理「组字串为空」的情况（此时应隐藏）。
    fn show(&mut self, state: &RenderState, anchor: Option<ScreenPoint>);
    fn hide(&mut self);
    fn is_visible(&self) -> bool;
}

/// 什么都不画的呈现器。用于测试，也用于「候选窗创建失败」的降级路径。
#[derive(Debug, Clone, Copy, Default)]
pub struct NullPresenter {
    visible: bool,
}

impl CandidatePresenter for NullPresenter {
    fn show(&mut self, state: &RenderState, _anchor: Option<ScreenPoint>) {
        self.visible = !state.is_empty();
    }
    fn hide(&mut self) {
        self.visible = false;
    }
    fn is_visible(&self) -> bool {
        self.visible
    }
}

/// 把状态位渲染成一行人类可读的标记。
pub fn format_status(s: StatusFlags) -> String {
    let mut parts: Vec<&str> = Vec::new();
    parts.push(if s.contains(StatusFlags::CHINESE) {
        "中"
    } else {
        "英"
    });
    if s.contains(StatusFlags::FULL_WIDTH_PUNCT) {
        parts.push("全角");
    }
    if s.contains(StatusFlags::CLOUD_BUSY) {
        parts.push("云端处理中");
    } else if s.contains(StatusFlags::CLOUD_OK) {
        parts.push("云");
    } else {
        parts.push("离线");
    }
    if s.contains(StatusFlags::VOICE_RECORDING) {
        parts.push("● 录音中");
    }
    if s.contains(StatusFlags::VOICE_OPTIMIZING) {
        parts.push("识别优化中");
    }
    if s.contains(StatusFlags::DEGRADED) {
        parts.push("降级");
    }
    parts.join(" · ")
}

fn source_tag(c: CandidateSource) -> &'static str {
    match c {
        CandidateSource::Local => " ",
        CandidateSource::SingleChar => "字",
        CandidateSource::User => "习",
        CandidateSource::Cloud => "云",
        CandidateSource::Hotword => "热",
    }
}

/// 把一次渲染状态转成多行文本。
///
/// 同时服务于 `retype-diag` 的终端输出和自动化测试的断言，
/// 保证「看到的」和「测到的」是同一份格式化逻辑。
pub fn format_state(state: &RenderState) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "[{}] {}\n",
        state.gen,
        format_status(state.status)
    ));

    if state.composition.is_empty() && state.candidates.is_empty() {
        out.push_str("  (空)\n");
        return out;
    }

    let syllables = if state.syllables.is_empty() {
        state.composition.clone()
    } else {
        // 已转换部分用文字显示，未转换部分显示音节切分
        let converted: String = state
            .composition
            .chars()
            .take(state.converted_len)
            .collect();
        format!("{converted}{}", state.syllables.join("'"))
    };
    out.push_str(&format!("  组字: {syllables}\n"));

    if state.candidates.is_empty() {
        out.push_str("  候选: (无)\n");
        return out;
    }

    let page = state.visible();
    let base = state.page_start;
    out.push_str("  候选:\n");
    for (i, c) in page.iter().enumerate() {
        let abs = base + i;
        let mark = if abs == state.selected { "▶" } else { " " };
        let key = if i < 9 {
            format!("{}", i + 1)
        } else {
            " ".into()
        };
        let comment = if c.comment.is_empty() {
            String::new()
        } else {
            format!("  <{}>", c.comment)
        };
        out.push_str(&format!(
            "   {mark} {key}. {}{} [{}]{comment}\n",
            c.text,
            " ".repeat(c.text.chars().count().min(1)),
            source_tag(c.source),
        ));
    }
    if state.page_count() > 1 {
        out.push_str(&format!(
            "  页 {}/{}  共 {} 条\n",
            state.page_start / state.page_size.max(1) + 1,
            state.page_count(),
            state.candidates.len()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use retype_types::{Candidate, CandidateSource, StatusFlags};

    fn state_with(texts: &[&str]) -> RenderState {
        RenderState {
            gen: 7,
            composition: "nihao".into(),
            converted_len: 0,
            syllables: vec!["ni".into(), "hao".into()],
            candidates: texts
                .iter()
                .map(|t| Candidate::new(*t, CandidateSource::Local))
                .collect(),
            selected: 0,
            page_size: 9,
            page_start: 0,
            status: StatusFlags::CHINESE.union(StatusFlags::CLOUD_OK),
        }
    }

    #[test]
    fn null_presenter_tracks_visibility() {
        let mut p = NullPresenter::default();
        assert!(!p.is_visible());
        p.show(&state_with(&["你好"]), None);
        assert!(p.is_visible());
        p.show(&RenderState::default(), None);
        assert!(!p.is_visible(), "空状态应自动隐藏");
        p.show(&state_with(&["你好"]), None);
        p.hide();
        assert!(!p.is_visible());
    }

    #[test]
    fn format_shows_composition_and_candidates() {
        let s = format_state(&state_with(&["你好", "你", "拟好"]));
        assert!(s.contains("ni'hao"), "{s}");
        assert!(s.contains("你好"));
        assert!(s.contains("拟好"));
        assert!(s.contains("▶"), "应标出当前高亮项");
    }

    #[test]
    fn status_line_reflects_mode_and_cloud() {
        assert_eq!(format_status(StatusFlags::CHINESE), "中 · 离线");
        assert!(format_status(StatusFlags::CHINESE.union(StatusFlags::CLOUD_OK)).contains("云"));
        assert!(
            format_status(StatusFlags::CHINESE.union(StatusFlags::CLOUD_BUSY))
                .contains("云端处理中")
        );
        assert!(format_status(StatusFlags::VOICE_OPTIMIZING).contains("识别优化中"));
        assert!(format_status(StatusFlags::DEGRADED).contains("降级"));
        assert!(format_status(StatusFlags::EMPTY).starts_with("英"));
    }

    #[test]
    fn converted_part_is_shown_as_text_not_pinyin() {
        let mut s = state_with(&["吗"]);
        s.composition = "你好ma".into();
        s.converted_len = 2;
        s.syllables = vec!["ma".into()];
        let out = format_state(&s);
        assert!(out.contains("你好ma"), "已选部分应显示为文字: {out}");
    }
}
