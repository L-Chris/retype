//! 统一输入与联想内核（test.md 图 5 的「大脑」）。
//!
//! **这是一个纯状态机**：`handle(InputEvent) -> Vec<KernelAction>`，
//! 不开线程、不做 IO、不碰平台 API。因此：
//! - 可以在终端里（`retype-diag`）直接跑，开发回路极短；
//! - 可以用单元测试精确断言每一次按键的后果；
//! - 并发（二刷、学习落盘、上下文采集）全部由 [`crate::backend::KernelBackend`] 提供，
//!   Windows 和 Android 各自实现自己的后端，内核一行都不用改。
//!
//! 这正是 P1（输入主线程绝不阻塞）能成立的结构性原因：内核里没有可以阻塞的东西。

use crate::merge::merge_outcome;
use retype_cloud::CloudClient;
use retype_pinyin::{DecodeOptions, Decoder, Lexicon};
use retype_types::{
    AppInfo, AsrEvent, Candidate, CommitRequest, ContextSnapshot, FieldInfo, Generation,
    InputEvent, InputSource, KernelAction, Key, LearningEvent, LearningStore, Modifiers,
    RenderState, RerankJob, RerankOutcome, SideEffect, StatusFlags, VoiceEvent,
};
use std::sync::Arc;

/// Chinese punctuation mapping, also used by platform key routing.
pub fn chinese_punctuation(c: char) -> Option<&'static str> {
    Some(match c {
        ',' => "，",
        '.' => "。",
        '?' => "？",
        '!' => "！",
        ':' => "：",
        ';' => "；",
        '(' => "（",
        ')' => "）",
        '[' => "【",
        ']' => "】",
        '<' => "《",
        '>' => "》",
        '\\' => "、",
        '^' => "……",
        '_' => "——",
        '$' => "￥",
        '`' => "·",
        '~' => "～",
        '"' => "“",
        '\'' => "‘",
        _ => return None,
    })
}

/// 内核配置。
#[derive(Debug, Clone)]
pub struct KernelConfig {
    pub pinyin_scheme: retype_types::PinyinScheme,
    pub decode: DecodeOptions,
    /// 是否启用二刷（断网/未配置云端时应关掉，省掉无谓的等待与打点）
    pub rerank_enabled: bool,
    pub chinese_on_start: bool,
    /// 组字串最大字符数，超过后不再接收字母（防止误触把整篇文章堆在组字区）
    pub max_buffer_chars: usize,
    /// 候选条数上限
    pub candidate_cap: usize,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self {
            pinyin_scheme: retype_types::PinyinScheme::Full,
            decode: DecodeOptions::default(),
            rerank_enabled: true,
            chinese_on_start: true,
            max_buffer_chars: 64,
            candidate_cap: 512,
        }
    }
}

/// 语音会话阶段（ARCHITECTURE.md §3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoicePhase {
    #[default]
    Idle,
    /// 正在采集，Interim/Stable 不断更新预览
    Recording,
    /// 已松手，等待 final pass（UI 显示「识别优化中」）
    Optimizing,
}

/// 组字串里已经选定的一段。保留原始拼音，是为了让退格能把字母**还原**回来，
/// 而不是把用户已经选好的词直接删掉。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Part {
    text: String,
    pinyin: String,
}

pub struct Kernel {
    cfg: KernelConfig,
    decoder: Decoder,
    dict: Arc<dyn Lexicon>,
    learner: Arc<dyn LearningStore>,
    cloud: Arc<CloudClient>,

    ctx: ContextSnapshot,
    gen: Generation,

    // ── 拼音组字 ──
    buffer: String,
    parts: Vec<Part>,
    candidates: Vec<Candidate>,
    syllables: Vec<String>,
    selected: usize,
    page_start: usize,
    page_starts: Vec<usize>,
    double_quote_open: bool,
    single_quote_open: bool,
    status: StatusFlags,
    rerank_inflight: Option<Generation>,

    // ── 语音 ──
    voice: VoicePhase,
    voice_text: String,
    voice_stable: String,
    voice_final: Option<String>,
}

impl std::fmt::Debug for Kernel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kernel")
            .field("gen", &self.gen)
            .field("buffer", &self.buffer)
            .field("parts", &self.parts)
            .field("candidates", &self.candidates.len())
            .field("voice", &self.voice)
            .field("status", &self.status)
            .finish()
    }
}

impl Kernel {
    pub fn new(
        cfg: KernelConfig,
        dict: Arc<dyn Lexicon>,
        learner: Arc<dyn LearningStore>,
        cloud: Arc<CloudClient>,
    ) -> Self {
        let mut status = StatusFlags::EMPTY;
        if cfg.chinese_on_start {
            status = status.union(StatusFlags::CHINESE);
        }
        if cloud.is_available() {
            status = status.union(StatusFlags::CLOUD_OK);
        }
        if dict.is_empty() {
            // 词库缺失 → 降级标记（ARCHITECTURE.md §7）
            status = status.union(StatusFlags::DEGRADED);
        }
        Self {
            decoder: Decoder::with_options(cfg.decode.clone()),
            cfg,
            dict,
            learner,
            cloud,
            ctx: ContextSnapshot::empty(),
            gen: 0,
            buffer: String::new(),
            parts: Vec::new(),
            candidates: Vec::new(),
            syllables: Vec::new(),
            selected: 0,
            page_start: 0,
            page_starts: Vec::new(),
            double_quote_open: false,
            single_quote_open: false,
            status,
            rerank_inflight: None,
            voice: VoicePhase::Idle,
            voice_text: String::new(),
            voice_stable: String::new(),
            voice_final: None,
        }
    }

    // ────────────────────────── 对外查询 ──────────────────────────

    pub fn generation(&self) -> Generation {
        self.gen
    }

    pub fn config(&self) -> &KernelConfig {
        &self.cfg
    }

    pub fn context(&self) -> &ContextSnapshot {
        &self.ctx
    }

    pub fn voice_phase(&self) -> VoicePhase {
        self.voice
    }

    pub fn is_chinese(&self) -> bool {
        self.status.contains(StatusFlags::CHINESE)
    }

    pub fn has_composition(&self) -> bool {
        !self.buffer.is_empty() || !self.parts.is_empty()
    }

    pub fn committed_text(&self) -> String {
        self.parts.iter().map(|p| p.text.as_str()).collect()
    }

    pub fn composition_text(&self) -> String {
        format!("{}{}", self.committed_text(), self.buffer)
    }

    pub fn converted_len(&self) -> usize {
        self.committed_text().chars().count()
    }

    /// 当前渲染状态。语音会话期间组字串显示识别文本。
    pub fn render_state(&self) -> RenderState {
        if self.voice != VoicePhase::Idle {
            let candidates = if self.voice_text.is_empty() {
                Vec::new()
            } else {
                vec![Candidate::new(
                    self.voice_text.clone(),
                    retype_types::CandidateSource::Cloud,
                )]
            };
            return RenderState {
                gen: self.gen,
                composition: self.voice_text.clone(),
                converted_len: 0,
                syllables: Vec::new(),
                candidates,
                selected: 0,
                page_size: self.cfg.decode.page_size,
                page_start: 0,
                page_starts: Vec::new(),
                status: self.status,
            };
        }
        RenderState {
            gen: self.gen,
            composition: self.composition_text(),
            converted_len: self.converted_len(),
            syllables: self.syllables.clone(),
            candidates: self.candidates.clone(),
            selected: self.selected,
            page_size: self.current_page_size(),
            page_start: self.page_start,
            page_starts: self.page_starts.clone(),
            status: self.status,
        }
    }

    // ────────────────────────── 主入口 ──────────────────────────

    /// 处理一个输入事件。**必须快速返回**（只做首刷），重活通过 `SideEffect` 外派。
    pub fn handle(&mut self, ev: InputEvent) -> Vec<KernelAction> {
        let mut actions: Vec<KernelAction> = Vec::with_capacity(4);
        match ev {
            InputEvent::Key { key, mods, source } => self.on_key(key, mods, source, &mut actions),
            InputEvent::ToggleChinese => self.on_toggle_chinese(&mut actions),
            InputEvent::SetPinyinScheme(scheme) => {
                if self.cfg.pinyin_scheme != scheme {
                    if self.has_composition() {
                        self.commit_raw_letters(&mut actions);
                    }
                    self.cfg.pinyin_scheme = scheme;
                    self.bump_gen();
                    actions.push(KernelAction::Render(self.render_state()));
                }
            }
            InputEvent::FocusChanged { app, field } => self.on_focus(app, field, &mut actions),
            InputEvent::ContextUpdated(snap) => self.on_context(snap),
            InputEvent::Voice(v) => self.on_voice(v, &mut actions),
            InputEvent::CandidateChosen { index } => self.choose(index, &mut actions),
            InputEvent::CandidatePage { delta } => self.page(delta, &mut actions),
            InputEvent::RerankCompleted { gen, result } => {
                self.on_rerank(gen, result, &mut actions)
            }
        }
        actions
    }

    // ────────────────────────── 按键 ──────────────────────────

    fn on_key(
        &mut self,
        key: Key,
        mods: Modifiers,
        source: InputSource,
        actions: &mut Vec<KernelAction>,
    ) {
        // 带 Ctrl/Alt/Win 的一律是宿主快捷键。若此时还留着组字串，
        // 必须先清掉 —— 否则宿主执行了快捷键，而屏幕上还挂着半截拼音。
        if !mods.is_plain() {
            if self.has_composition() {
                self.cancel_composition(actions);
            }
            actions.push(KernelAction::PassThrough);
            return;
        }

        // Shift+字母 = 用户想打大写英文
        if mods.contains(Modifiers::SHIFT) {
            if let Key::Char(c) = key {
                if c.is_ascii_alphanumeric() {
                    if self.has_composition() {
                        self.commit_raw_letters(actions);
                    }
                    actions.push(KernelAction::PassThrough);
                    return;
                }
            }
        }

        if !self.is_chinese() {
            // 英文模式：全部直通，输入法不参与
            actions.push(KernelAction::PassThrough);
            return;
        }

        match key {
            Key::Char(c) if c.is_ascii_alphabetic() => {
                if self.buffer.chars().count() >= self.cfg.max_buffer_chars {
                    return;
                }
                self.buffer.push(c.to_ascii_lowercase());
                self.bump_gen();
                self.redecode(source, actions);
            }
            Key::Char('\'') if !self.buffer.is_empty() => {
                // 显式音节分隔符：xi'an vs xian
                if self.buffer.ends_with('\'') {
                    return;
                }
                self.buffer.push('\'');
                self.bump_gen();
                self.redecode(source, actions);
            }
            Key::Char(d @ '0'..='9') => {
                if self.has_composition() && d != '0' {
                    let idx = self.page_start + (d as usize - '1' as usize);
                    if (d as usize - '1' as usize) < self.current_page_size() {
                        self.choose(idx, actions);
                    }
                } else {
                    actions.push(KernelAction::PassThrough);
                }
            }
            Key::Space => {
                if self.has_composition() {
                    let idx = self.selected;
                    self.choose(idx, actions);
                } else {
                    actions.push(KernelAction::PassThrough);
                }
            }
            Key::Backspace => self.on_backspace(source, actions),
            Key::Enter => {
                if self.has_composition() {
                    // 回车 = 上屏原始字母，这是用户「我就要打英文」的逃生通道
                    self.commit_raw_letters(actions);
                } else {
                    actions.push(KernelAction::PassThrough);
                }
            }
            Key::Escape => {
                if self.has_composition() {
                    self.cancel_composition(actions);
                } else {
                    actions.push(KernelAction::PassThrough);
                }
            }
            Key::Left => self.move_selection(-1, actions),
            Key::Right => self.move_selection(1, actions),
            Key::Up => self.page(-1, actions),
            Key::Down => self.page(1, actions),
            Key::PageUp => self.page(-1, actions),
            Key::PageDown => self.page(1, actions),
            Key::Char('-') => self.page(-1, actions),
            Key::Char('=') => self.page(1, actions),
            // 中文标点与首选一起上屏；未映射的符号交回宿主。
            Key::Char(c) => {
                let punctuation = match c {
                    '"' => {
                        self.double_quote_open = !self.double_quote_open;
                        Some(if self.double_quote_open { "“" } else { "”" })
                    }
                    '\'' => {
                        self.single_quote_open = !self.single_quote_open;
                        Some(if self.single_quote_open { "‘" } else { "’" })
                    }
                    _ => chinese_punctuation(c),
                };
                if self.has_composition() {
                    self.commit_best(actions);
                    if let Some(mark) = punctuation {
                        for action in actions.iter_mut().rev() {
                            if let KernelAction::Commit(CommitRequest::ReplaceComposition {
                                text,
                            }) = action
                            {
                                text.push_str(mark);
                                return;
                            }
                        }
                    }
                }
                if let Some(mark) = punctuation {
                    actions.push(KernelAction::Commit(CommitRequest::Text(mark.into())));
                } else {
                    actions.push(KernelAction::PassThrough);
                }
            }
            _ => {
                if self.has_composition() {
                    self.cancel_composition(actions);
                }
                actions.push(KernelAction::PassThrough);
            }
        }
    }

    fn on_backspace(&mut self, source: InputSource, actions: &mut Vec<KernelAction>) {
        if !self.buffer.is_empty() {
            self.buffer.pop();
            // 末尾留下孤立的分隔符没有意义
            while self.buffer.ends_with('\'')
                && self.buffer.chars().filter(|c| *c != '\'').count() == 0
            {
                self.buffer.pop();
            }
            self.bump_gen();
            self.redecode(source, actions);
            return;
        }
        if let Some(last) = self.parts.pop() {
            // 把已选词还原成拼音，让用户能改主意（而不是直接删掉他选好的字）
            self.buffer = last.pinyin;
            self.bump_gen();
            self.redecode(source, actions);
            return;
        }
        actions.push(KernelAction::PassThrough);
    }

    fn on_toggle_chinese(&mut self, actions: &mut Vec<KernelAction>) {
        self.double_quote_open = false;
        self.single_quote_open = false;
        if self.has_composition() {
            self.commit_raw_letters(actions);
        }
        self.status = if self.is_chinese() {
            self.status.difference(StatusFlags::CHINESE)
        } else {
            self.status.union(StatusFlags::CHINESE)
        };
        self.bump_gen();
        actions.push(KernelAction::Render(self.render_state()));
    }

    fn on_focus(&mut self, app: AppInfo, field: FieldInfo, actions: &mut Vec<KernelAction>) {
        self.double_quote_open = false;
        self.single_quote_open = false;
        // 焦点变了就**丢弃**组字串，绝不上屏。
        // test.md 第六节列的头号事故就是「文字插入到错误窗口」。
        self.reset_composition();
        self.bump_gen();
        self.ctx = ContextSnapshot {
            app,
            field,
            ..ContextSnapshot::empty()
        };
        actions.push(KernelAction::Side(SideEffect::CollectContext));
        actions.push(KernelAction::Render(self.render_state()));
    }

    fn on_context(&mut self, snap: ContextSnapshot) {
        // M0：上下文只影响二刷，不影响本地打分，所以不重解码
        //（重解码会让用户正在看的候选突然跳变）。M2 引入本地上下文打分后再评估。
        self.ctx = snap;
    }

    // ────────────────────────── 组字推进 ──────────────────────────

    fn bump_gen(&mut self) {
        self.gen = self.gen.wrapping_add(1);
        // 代次变了，在飞的二刷结果一律作废
        self.rerank_inflight = None;
        self.status = self.status.difference(StatusFlags::CLOUD_BUSY);
    }

    /// 首刷：本地解码 + 渲染 + 派发二刷。
    fn redecode(&mut self, source: InputSource, actions: &mut Vec<KernelAction>) {
        self.page_starts.clear();
        if self.buffer.is_empty() {
            self.candidates.clear();
            self.syllables.clear();
            self.selected = 0;
            self.page_start = 0;
            actions.push(KernelAction::Render(self.render_state()));
            return;
        }
        let out = match self.cfg.pinyin_scheme {
            retype_types::PinyinScheme::Full => {
                self.decoder.decode(&self.buffer, self.dict.as_ref())
            }
            retype_types::PinyinScheme::Flypy => {
                retype_pinyin::shuangpin::decode(&self.buffer, self.dict.as_ref(), &self.cfg.decode)
            }
        };
        self.candidates = out.candidates;
        // Raw paths keep the decoder total, but the candidate window must show
        // only text backed by a dictionary match. Unmatched keys stay underlined
        // in the composition and can still be committed with Enter.
        self.candidates
            .retain(|c| c.source != retype_types::CandidateSource::Raw);
        self.candidates.truncate(self.cfg.candidate_cap);
        self.syllables = out.syllables;
        self.selected = 0;
        self.page_start = 0;

        self.maybe_schedule_rerank(source, actions);
        actions.push(KernelAction::Render(self.render_state()));
    }

    fn maybe_schedule_rerank(&mut self, source: InputSource, actions: &mut Vec<KernelAction>) {
        if !self.cfg.rerank_enabled || self.buffer.is_empty() || self.candidates.is_empty() {
            return;
        }
        if self.rerank_inflight == Some(self.gen) {
            return;
        }
        if !self.cloud.is_available() {
            // 熔断中：连请求都不发，避免每次按键白等一个超时
            self.status = self.status.difference(StatusFlags::CLOUD_OK);
            return;
        }
        self.rerank_inflight = Some(self.gen);
        self.status = self.status.union(StatusFlags::CLOUD_BUSY);
        actions.push(KernelAction::Side(SideEffect::Rerank(RerankJob {
            gen: self.gen,
            source,
            composition: self.buffer.clone(),
            syllables: self.syllables.clone(),
            candidates: self.candidates.clone(),
            context: self.ctx.clone(),
        })));
    }

    /// 二刷结果回来了。
    fn on_rerank(
        &mut self,
        gen: Generation,
        outcome: RerankOutcome,
        actions: &mut Vec<KernelAction>,
    ) {
        self.rerank_inflight = None;
        self.status = self.status.difference(StatusFlags::CLOUD_BUSY);

        // 代次校验：用户已经继续输入了，这份结果整包丢弃。
        // 没有这一步，快速打字时候选会「往回跳」。
        if gen != self.gen {
            return;
        }
        if outcome.degraded {
            self.status = self.status.difference(StatusFlags::CLOUD_OK);
            return; // 首刷结果原样保留（P3）
        }
        self.status = self.status.union(StatusFlags::CLOUD_OK);

        let before = self.selected_text();
        self.candidates = merge_outcome(&self.candidates, &outcome);
        self.candidates.truncate(self.cfg.candidate_cap);
        // 高亮跟着「同一个词」走，而不是跟着下标走 —— 否则重排后高亮会跳到别的词上
        if let Some(text) = before {
            if let Some(i) = self.candidates.iter().position(|c| c.text == text) {
                self.set_selected(i);
            }
        }
        actions.push(KernelAction::Render(self.render_state()));
    }

    fn selected_text(&self) -> Option<String> {
        self.candidates.get(self.selected).map(|c| c.text.clone())
    }

    fn choose(&mut self, index: usize, actions: &mut Vec<KernelAction>) {
        let Some(c) = self.candidates.get(index).cloned() else {
            return;
        };
        let cut = c.consumed.min(self.buffer.chars().count());
        // `consumed` 是字符数，而 buffer 是 ASCII，字节数 == 字符数
        let cut_bytes = cut.min(self.buffer.len());
        let pinyin = self.buffer[..cut_bytes].to_string();

        actions.push(KernelAction::Side(SideEffect::Learn(
            LearningEvent::CandidateChosen {
                source: InputSource::Keyboard,
                text: c.text.clone(),
                syllables: c.syllables.clone(),
                index,
            },
        )));

        self.buffer.drain(..cut_bytes);
        self.parts.push(Part {
            text: c.text.clone(),
            pinyin,
        });
        self.bump_gen();

        if self.buffer.is_empty() {
            let text = self.committed_text();
            self.reset_composition();
            actions.push(KernelAction::Commit(CommitRequest::ReplaceComposition {
                text,
            }));
            actions.push(KernelAction::Render(self.render_state()));
        } else {
            self.redecode(InputSource::Keyboard, actions);
        }
    }

    /// 上屏首选（整句），用于「组字中直接打标点」的场景。
    fn commit_best(&mut self, actions: &mut Vec<KernelAction>) {
        let text = match self.candidates.first() {
            Some(c) => {
                let remainder = if c.consumed == 0 {
                    ""
                } else {
                    &self.buffer[c.consumed.min(self.buffer.len())..]
                };
                format!("{}{}{}", self.committed_text(), c.text, remainder)
            }
            None => self.composition_text(),
        };
        self.reset_composition();
        self.bump_gen();
        actions.push(KernelAction::Commit(CommitRequest::ReplaceComposition {
            text,
        }));
        actions.push(KernelAction::Render(self.render_state()));
    }

    /// 上屏原始字母（回车的逃生通道）。
    fn commit_raw_letters(&mut self, actions: &mut Vec<KernelAction>) {
        let text = self.composition_text();
        self.reset_composition();
        self.bump_gen();
        if text.is_empty() {
            return;
        }
        actions.push(KernelAction::Commit(CommitRequest::ReplaceComposition {
            text,
        }));
        actions.push(KernelAction::Render(self.render_state()));
    }

    fn cancel_composition(&mut self, actions: &mut Vec<KernelAction>) {
        self.reset_composition();
        self.bump_gen();
        actions.push(KernelAction::Render(self.render_state()));
    }

    fn reset_composition(&mut self) {
        self.buffer.clear();
        self.parts.clear();
        self.candidates.clear();
        self.page_starts.clear();
        self.syllables.clear();
        self.selected = 0;
        self.page_start = 0;
        self.rerank_inflight = None;
        self.status = self.status.difference(StatusFlags::CLOUD_BUSY);
    }

    // ────────────────────────── 候选窗导航 ──────────────────────────

    /// Apply measured widths without dropping or reordering candidates.
    pub fn layout_candidates(&mut self, widths: &[i32], available: i32, gap: i32) {
        if widths.len() != self.candidates.len() {
            return;
        }
        self.page_starts.clear();
        let mut used = 0;
        let mut count = 0;
        for (index, &width) in widths.iter().enumerate() {
            if count == 0
                || count >= self.cfg.decode.page_size.max(1)
                || used + gap + width > available
            {
                self.page_starts.push(index);
                used = width;
                count = 1;
            } else {
                used += gap + width;
                count += 1;
            }
        }
        self.set_selected(self.selected);
    }

    fn current_page_size(&self) -> usize {
        if self.page_starts.is_empty() {
            return self.cfg.decode.page_size.max(1);
        }
        self.page_starts
            .iter()
            .copied()
            .find(|&start| start > self.page_start)
            .unwrap_or(self.candidates.len())
            .saturating_sub(self.page_start)
            .max(1)
    }

    fn set_selected(&mut self, i: usize) {
        let n = self.candidates.len();
        if n == 0 {
            self.selected = 0;
            self.page_start = 0;
            return;
        }
        self.selected = i.min(n - 1);
        if !self.page_starts.is_empty() {
            self.page_start = *self
                .page_starts
                .iter()
                .rev()
                .find(|&&start| start <= self.selected)
                .unwrap_or(&0);
            return;
        }
        let size = self.cfg.decode.page_size.max(1);
        if self.selected < self.page_start {
            self.page_start = self.selected;
        } else if self.selected >= self.page_start + size {
            self.page_start = self.selected - size + 1;
        }
    }

    fn move_selection(&mut self, delta: i32, actions: &mut Vec<KernelAction>) {
        if self.candidates.is_empty() {
            actions.push(KernelAction::PassThrough);
            return;
        }
        let n = self.candidates.len() as i32;
        let next = (self.selected as i32 + delta).rem_euclid(n) as usize;
        self.set_selected(next);
        actions.push(KernelAction::Render(self.render_state()));
    }

    fn page(&mut self, delta: i32, actions: &mut Vec<KernelAction>) {
        if self.candidates.is_empty() {
            actions.push(KernelAction::PassThrough);
            return;
        }
        if !self.page_starts.is_empty() {
            let current = self
                .page_starts
                .partition_point(|&start| start <= self.page_start)
                .saturating_sub(1);
            let next =
                (current as i32 + delta).clamp(0, self.page_starts.len() as i32 - 1) as usize;
            self.page_start = self.page_starts[next];
            self.selected = self.page_start;
            actions.push(KernelAction::Render(self.render_state()));
            return;
        }
        let size = self.cfg.decode.page_size.max(1) as i32;
        let pages = ((self.candidates.len() as i32) + size - 1) / size;
        let cur = (self.page_start as i32) / size;
        let next = (cur + delta).clamp(0, (pages - 1).max(0)) as usize;
        self.page_start = (next * size as usize).min(self.candidates.len().saturating_sub(1));
        self.selected = self.page_start;
        actions.push(KernelAction::Render(self.render_state()));
    }

    // ────────────────────────── 语音（三段式） ──────────────────────────

    fn on_voice(&mut self, ev: VoiceEvent, actions: &mut Vec<KernelAction>) {
        match ev {
            VoiceEvent::Start => {
                // 语音开始前先把拼音组字丢掉，两种输入方式不能同时占着组字区
                self.reset_composition();
                self.voice = VoicePhase::Recording;
                self.voice_text.clear();
                self.voice_stable.clear();
                self.voice_final = None;
                self.status = self
                    .status
                    .difference(StatusFlags::VOICE_OPTIMIZING)
                    .union(StatusFlags::VOICE_RECORDING);
                self.bump_gen();
                actions.push(KernelAction::Render(self.render_state()));
            }
            VoiceEvent::Stop => {
                self.status = self.status.difference(StatusFlags::VOICE_RECORDING);
                if let Some(t) = self.voice_final.clone() {
                    // final 已经到了，不必再等（test.md：松手后不该无谓地显示「优化中」）
                    self.commit_voice(&t, actions);
                } else if self.voice_text.is_empty() {
                    // 什么都没说 → 静默收场
                    self.reset_voice();
                    self.bump_gen();
                    actions.push(KernelAction::Render(self.render_state()));
                } else {
                    // 关键：松手不等于结束。进入 Optimizing 等 final pass，
                    // 由平台层的定时器投递 OptimizeTimeout 兜底。
                    self.voice = VoicePhase::Optimizing;
                    self.status = self.status.union(StatusFlags::VOICE_OPTIMIZING);
                    self.bump_gen();
                    actions.push(KernelAction::Render(self.render_state()));
                }
            }
            VoiceEvent::OptimizeTimeout => {
                if self.voice != VoicePhase::Optimizing {
                    return;
                }
                // 兜底：宁可用稳定段，也绝不让用户的字消失
                let t = if self.voice_stable.is_empty() {
                    self.voice_text.clone()
                } else {
                    self.voice_stable.clone()
                };
                if t.is_empty() {
                    self.reset_voice();
                    self.bump_gen();
                    actions.push(KernelAction::Render(self.render_state()));
                } else {
                    self.commit_voice(&t, actions);
                }
            }
            VoiceEvent::Cancel => {
                self.reset_voice();
                self.bump_gen();
                actions.push(KernelAction::Render(self.render_state()));
            }
            VoiceEvent::Asr(asr) => self.on_asr(asr, actions),
        }
    }

    fn on_asr(&mut self, ev: AsrEvent, actions: &mut Vec<KernelAction>) {
        match ev {
            // 第一遍：实时听写，只是预览，随时会被改写
            AsrEvent::Interim(t) => {
                self.voice_text = t;
                self.bump_gen();
                actions.push(KernelAction::Render(self.render_state()));
            }
            // 第二遍：停顿后的稳定段。它会覆盖前面的文字（test.md 图 2）
            AsrEvent::Stable(t) => {
                self.voice_stable = t.clone();
                self.voice_text = t;
                self.bump_gen();
                actions.push(KernelAction::Render(self.render_state()));
            }
            // 第三遍：整段定稿
            AsrEvent::Final(t) => {
                self.voice_final = Some(t.clone());
                self.voice_text = t.clone();
                self.bump_gen();
                if self.voice == VoicePhase::Optimizing {
                    self.commit_voice(&t, actions);
                } else {
                    actions.push(KernelAction::Render(self.render_state()));
                }
            }
        }
    }

    fn commit_voice(&mut self, text: &str, actions: &mut Vec<KernelAction>) {
        if text.is_empty() {
            self.reset_voice();
            self.bump_gen();
            actions.push(KernelAction::Render(self.render_state()));
            return;
        }
        let owned = text.to_owned();
        self.reset_voice();
        self.bump_gen();
        actions.push(KernelAction::Commit(CommitRequest::Text(owned.clone())));
        // 语音上屏的词要反哺拼音候选（test.md 图 5 的闭环）
        actions.push(KernelAction::Side(SideEffect::Learn(
            LearningEvent::VoiceCommit { text: owned },
        )));
        actions.push(KernelAction::Render(self.render_state()));
    }

    fn reset_voice(&mut self) {
        self.voice = VoicePhase::Idle;
        self.voice_text.clear();
        self.voice_stable.clear();
        self.voice_final = None;
        self.status = self
            .status
            .difference(StatusFlags::VOICE_RECORDING)
            .difference(StatusFlags::VOICE_OPTIMIZING);
    }

    // ────────────────────────── 诊断 ──────────────────────────

    /// 只读地看一眼当前候选文字，`retype-diag` 用。
    pub fn candidate_texts(&self) -> Vec<&str> {
        self.candidates.iter().map(|c| c.text.as_str()).collect()
    }

    /// 学习器句柄（后端执行 `SideEffect::Learn` 时用）。
    pub fn learner(&self) -> &Arc<dyn LearningStore> {
        &self.learner
    }

    /// 云端可用性变化时刷新状态位。
    pub fn refresh_cloud_status(&mut self) {
        self.status = if self.cloud.is_available() {
            self.status.union(StatusFlags::CLOUD_OK)
        } else {
            self.status.difference(StatusFlags::CLOUD_OK)
        };
    }
}
