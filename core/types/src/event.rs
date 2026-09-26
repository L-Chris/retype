//! 输入事件：所有输入方式（物理键盘 / 语音 / 触摸键盘）统一翻译成这里的类型。
//!
//! 对应 ARCHITECTURE.md §0（test.md 图 5：多种输入方式共用同一个内核）。

/// 输入来源。只作为标签参与学习记录，不改变内核逻辑。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputSource {
    Keyboard,
    Voice,
    Touch,
}

/// 修饰键，位掩码实现（不引入 bitflags 依赖）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const NONE: Self = Self(0);
    pub const SHIFT: Self = Self(1 << 0);
    pub const CTRL: Self = Self(1 << 1);
    pub const ALT: Self = Self(1 << 2);
    pub const WIN: Self = Self(1 << 3);

    #[inline]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[inline]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[inline]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    #[inline]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    #[inline]
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }

    /// 是否只带 Shift（大小写/符号），即「不是快捷键组合」。
    #[inline]
    pub const fn is_plain(self) -> bool {
        self.0 & (Self::CTRL.0 | Self::ALT.0 | Self::WIN.0) == 0
    }
}

/// 抽象按键。平台层负责把 `wParam`/`KeyEvent` 映射到这里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// 可打印字符（已小写化由平台层决定，内核会自行归一）
    Char(char),
    Backspace,
    Delete,
    Enter,
    Escape,
    Space,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

/// 语音链路事件。M0 只定义形状，M4 接真实 ASR（见 docs/adr/0002）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEvent {
    /// 按下热键，开始采集
    Start,
    /// 松开热键：必须发 AudioEnd 并等待 final pass（ARCHITECTURE.md §3）
    Stop,
    /// 主动取消，丢弃全部结果
    Cancel,
    /// 松手后等待 final pass 超时：用最后一个 Stable 兜底上屏，
    /// 绝不让用户的字消失（ARCHITECTURE.md §3 硬要求 2）
    OptimizeTimeout,
    /// 来自 ASR 的识别事件
    Asr(AsrEvent),
}

/// ASR 三段式事件，对应 test.md 图 2。
///
/// 命名刻意与供应商无关：任何流式 ASR 都能映射到这三档。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsrEvent {
    /// 实时临时结果：可变，仅用于即时反馈
    Interim(String),
    /// 停顿后的稳定段（two-pass）：可以改写前面的文字
    Stable(String),
    /// 松手后的整段定稿（final pass）
    Final(String),
}

/// 内核的唯一输入。平台适配层只做「系统事件 → InputEvent」的翻译。
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    Key {
        key: Key,
        mods: Modifiers,
        source: InputSource,
    },
    /// 焦点变化（切换输入框 / 切换应用），内核据此清理组字状态
    FocusChanged {
        app: super::AppInfo,
        field: super::FieldInfo,
    },
    /// 上下文采集完成（异步，可能失败 → 空快照）
    ContextUpdated(super::ContextSnapshot),
    /// 中英切换（Shift / 热键 / 语言栏点击）。平台层判定后投递，内核不猜按键组合。
    ToggleChinese,
    Voice(VoiceEvent),
    /// 用户从候选窗选了第 index 个
    CandidateChosen {
        index: usize,
    },
    /// 翻页
    CandidatePage {
        delta: i32,
    },
    /// 二刷结果回来了。携带发出时的 gen，过期则由内核丢弃
    RerankCompleted {
        gen: super::Generation,
        result: super::RerankOutcome,
    },
}
