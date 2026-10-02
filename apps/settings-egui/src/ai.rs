//! Provider, translation and shortcut forms. Network work never runs in the UI callback.
use eframe::egui::{self, RichText};
use retype_ai::{
    config::{ApiKind, Config, Provider, Shortcut, Shortcuts, PRESETS},
    protocol::{Operation, Response},
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
struct Job {
    id: String,
    generation: u64,
    cancelled: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<Response, String>>,
}
pub struct AiPages {
    pub config: Config,
    pub shortcuts: Shortcuts,
    keys: HashMap<String, String>,
    models: HashMap<String, Vec<String>>,
    manual: HashMap<String, String>,
    messages: HashMap<String, String>,
    job: Option<Job>,
    generation: u64,
    dirty: Option<Instant>,
    saved: Config,
    saved_keys: HashMap<String, String>,
    capture: Option<bool>,
    tap: u16,
    error: Option<String>,
    languages: String,
    test_text: String,
    test_output: String,
}
impl AiPages {
    pub fn cancel_capture(&mut self) {
        self.capture = None;
        self.tap = 0;
    }
    pub fn new() -> Self {
        let loaded = Config::load();
        let mut error = loaded
            .as_ref()
            .err()
            .map(|e| format!("无法读取 AI 配置：{e}"));
        let config = loaded.unwrap_or_default();
        let keys = config
            .providers
            .iter()
            .map(|p| {
                let key = retype_ai::secrets::key(&p.id).unwrap_or_else(|_| {
                    error = Some("部分 API Key 无法读取，请检查 Windows 凭据权限".into());
                    String::new()
                });
                (p.id.clone(), key)
            })
            .collect::<HashMap<_, _>>();
        Self::from_config(config, keys, retype_ai::secrets::shortcuts(), error)
    }
    fn from_config(
        config: Config,
        keys: HashMap<String, String>,
        shortcuts: Shortcuts,
        error: Option<String>,
    ) -> Self {
        Self {
            saved: config.clone(),
            saved_keys: keys.clone(),
            config,
            keys,
            models: HashMap::new(),
            manual: HashMap::new(),
            messages: HashMap::new(),
            shortcuts,
            job: None,
            generation: 0,
            dirty: None,
            capture: None,
            tap: 0,
            error,
            languages: String::new(),
            test_text: String::new(),
            test_output: String::new(),
        }
    }
    pub fn flush(&mut self) -> Result<(), String> {
        if self.config == self.saved && self.keys == self.saved_keys {
            self.dirty = None;
            return Ok(());
        }
        for (id, key) in &self.keys {
            if self.saved_keys.get(id) != Some(key) {
                retype_ai::secrets::save_key(id, key)
                    .map_err(|_| "无法保存 API Key".to_string())?;
            }
        }
        self.config
            .save()
            .map_err(|e| format!("无法保存 AI 配置：{e}"))?;
        for id in self.saved_keys.keys() {
            if !self.keys.contains_key(id) {
                let _ = retype_ai::secrets::save_key(id, "");
            }
        }
        self.saved = self.config.clone();
        self.saved_keys = self.keys.clone();
        self.dirty = None;
        Ok(())
    }
    pub fn tick(&mut self, ctx: &egui::Context) {
        let received = self.job.as_ref().and_then(|j| j.receiver.try_recv().ok());
        if let Some(result) = received {
            if let Some(job) = self.job.take() {
                if job.generation == self.generation {
                    match result {
                        Ok(Response::Models(models)) => {
                            self.models.insert(job.id.clone(), models);
                            self.messages.remove(&job.id);
                        }
                        Ok(Response::Tested) => {
                            self.messages.insert(job.id, "连接成功".into());
                        }
                        Ok(Response::Translation { text, .. }) => {
                            self.test_output = text;
                            self.messages.remove(&job.id);
                        }
                        Err(e) => {
                            self.messages.insert(job.id, e);
                        }
                        _ => {}
                    }
                }
            }
        }
        if self.config != self.saved || self.keys != self.saved_keys {
            let dirty = self.dirty.get_or_insert_with(Instant::now);
            if dirty.elapsed() > Duration::from_millis(700) {
                if let Err(e) = self.flush() {
                    self.error = Some(e);
                    self.dirty = Some(Instant::now());
                }
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
    fn invalidate(&mut self, id: &str) {
        self.generation = self.generation.wrapping_add(1);
        self.models.remove(id);
        self.messages.remove(id);
        if let Some(job) = self.job.take() {
            job.cancelled.store(true, Ordering::Relaxed);
        }
    }
    fn start(&mut self, id: String, op: Operation, ctx: &egui::Context) {
        if let Err(e) = self.flush() {
            self.error = Some(e);
            return;
        }
        if let Some(job) = self.job.take() {
            job.cancelled.store(true, Ordering::Relaxed);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancelled);
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.messages.remove(&id);
        match std::thread::Builder::new()
            .name("retype-ai-settings".into())
            .spawn(move || {
                let _ = tx.send(retype_ai::client::run(op, &worker_cancel));
                ctx.request_repaint();
            }) {
            Ok(_) => {
                self.job = Some(Job {
                    id,
                    generation: self.generation,
                    cancelled,
                    receiver: rx,
                })
            }
            Err(_) => self.error = Some("无法创建后台任务".into()),
        }
    }
    pub fn providers(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        let mut action = None;
        for index in 0..self.config.providers.len() {
            let before = self.config.providers[index].clone();
            let id = before.id.clone();
            let before_key = self.keys.get(&id).cloned().unwrap_or_default();
            let loading = self.job.as_ref().is_some_and(|j| j.id == id);
            let available = self.models.get(&id).cloned().unwrap_or_default();
            let message = self.messages.get(&id).cloned();
            super::app::card(ui, |ui| {
                let field_width = (ui.available_width() - 190.0).clamp(120.0, 280.0);
                configure_form(ui);
                let p = &mut self.config.providers[index];
                egui::Grid::new(("provider", &id))
                    .num_columns(2)
                    .min_row_height(FORM_HEIGHT)
                    .spacing([20.0, 12.0])
                    .show(ui, |ui| {
                        provider_label(ui, "提供商");
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt(("preset", &id))
                                .width(field_width)
                                .truncate()
                                .selected_text(&p.preset)
                                .show_ui(ui, |ui| {
                                    let search_id = ui.id().with("search");
                                    let mut search = ui
                                        .data_mut(|d| d.get_temp::<String>(search_id))
                                        .unwrap_or_default();
                                    ui.add(
                                        egui::TextEdit::singleline(&mut search)
                                            .hint_text("搜索提供商"),
                                    );
                                    ui.data_mut(|d| d.insert_temp(search_id, search.clone()));
                                    for &(name, url, kind) in PRESETS {
                                        if !name.to_lowercase().contains(&search.to_lowercase()) {
                                            continue;
                                        }
                                        if ui.selectable_label(p.preset == name, name).clicked() {
                                            p.preset = name.into();
                                            p.name = name.into();
                                            p.kind = kind;
                                            p.base_url = url.into();
                                            p.models.clear();
                                        }
                                    }
                                });
                            if ui.button("删除").clicked() {
                                remove = Some(index);
                            }
                        });
                        ui.end_row();
                        if p.preset == "自定义" {
                            provider_label(ui, "名称");
                            provider_text(ui, &mut p.name, field_width, false);
                            ui.end_row();
                            provider_label(ui, "接口类型");
                            egui::ComboBox::from_id_salt(("api-kind", &id))
                                .width(field_width)
                                .truncate()
                                .selected_text(kind_name(p.kind))
                                .show_ui(ui, |ui| {
                                    for kind in [
                                        ApiKind::Compatible,
                                        ApiKind::Anthropic,
                                        ApiKind::Gemini,
                                        ApiKind::Ollama,
                                    ] {
                                        ui.selectable_value(&mut p.kind, kind, kind_name(kind));
                                    }
                                });
                            ui.end_row();
                        }
                        provider_label(ui, "接口地址");
                        provider_text(ui, &mut p.base_url, field_width, false);
                        ui.end_row();
                        provider_label(ui, "API Key");
                        provider_text(
                            ui,
                            self.keys.entry(id.clone()).or_default(),
                            field_width,
                            true,
                        );
                        ui.end_row();
                        provider_label(ui, "模型");
                        ui.horizontal(|ui| {
                            let label = match p.models.as_slice() {
                                [] => "请选择模型".into(),
                                [model] => model.clone(),
                                models => format!("已选择 {} 个模型", models.len()),
                            };
                            egui::ComboBox::from_id_salt(("models", &id))
                                .width(field_width)
                                .truncate()
                                .selected_text(label)
                                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                                .show_ui(ui, |ui| {
                                    let search_id = ui.id().with("search");
                                    let mut search = ui
                                        .data_mut(|d| d.get_temp::<String>(search_id))
                                        .unwrap_or_default();
                                    ui.add(
                                        egui::TextEdit::singleline(&mut search)
                                            .hint_text("搜索模型"),
                                    );
                                    ui.data_mut(|d| d.insert_temp(search_id, search.clone()));
                                    let mut options = available.clone();
                                    options.extend(p.models.clone());
                                    options.sort();
                                    options.dedup();
                                    if options.is_empty() {
                                        ui.weak(if loading {
                                            "正在获取模型…"
                                        } else {
                                            "刷新列表或手动添加模型"
                                        });
                                    }
                                    for model in options {
                                        if !model.to_lowercase().contains(&search.to_lowercase()) {
                                            continue;
                                        }
                                        let selected = p.models.contains(&model);
                                        if ui.selectable_label(selected, &model).clicked() {
                                            if selected {
                                                p.models.retain(|m| m != &model);
                                            } else {
                                                p.models.push(model);
                                            }
                                        }
                                    }
                                    ui.separator();
                                    ui.horizontal(|ui| {
                                        let manual = self.manual.entry(id.clone()).or_default();
                                        ui.add(
                                            egui::TextEdit::singleline(manual)
                                                .desired_width(200.0)
                                                .hint_text("模型 ID"),
                                        );
                                        if ui.button("添加").clicked() && !manual.trim().is_empty()
                                        {
                                            let model = manual.trim().to_owned();
                                            if !p.models.contains(&model) {
                                                p.models.push(model);
                                            }
                                            manual.clear();
                                        }
                                    });
                                });
                            if ui
                                .add_enabled(!loading, egui::Button::new("刷新"))
                                .clicked()
                            {
                                action = Some((
                                    id.clone(),
                                    Operation::Models {
                                        provider: p.clone(),
                                    },
                                ));
                            }
                            if loading {
                                ui.spinner();
                            }
                        });
                        ui.end_row();
                        provider_label(ui, "连接测试");
                        if ui
                            .add_enabled(
                                !loading && !p.models.is_empty(),
                                egui::Button::new("测试连接"),
                            )
                            .clicked()
                        {
                            action = Some((
                                id.clone(),
                                Operation::Test {
                                    provider: p.clone(),
                                    model: p.models.first().cloned().unwrap_or_default(),
                                },
                            ));
                        }
                        ui.end_row();
                    });
                if let Some(message) = &message {
                    ui.label(message);
                }
            });
            let key = self.keys.get(&id).cloned().unwrap_or_default();
            let p = &self.config.providers[index];
            let provider_changed =
                before.base_url != p.base_url || before.kind != p.kind || before_key != key;
            if provider_changed {
                self.dirty = Some(Instant::now());
                self.invalidate(&id);
            }
            let provider = self.config.providers[index].clone();
            if action.is_none()
                && !provider_changed
                && self.dirty.is_none()
                && self.job.is_none()
                && !self.models.contains_key(&id)
                && !self.messages.contains_key(&id)
                && !provider.base_url.trim().is_empty()
                && (!key.is_empty() || provider.kind == ApiKind::Ollama)
            {
                action = Some((id.clone(), Operation::Models { provider }));
            }
            ui.add_space(8.0);
        }
        if let Some(index) = remove {
            let removed = self.config.providers.remove(index);
            self.keys.remove(&removed.id);
            self.invalidate(&removed.id);
            if self.config.provider == removed.id {
                self.config.provider.clear();
                self.config.model.clear();
            }
        }
        if ui.button("添加提供商").clicked() {
            let p = Provider::default();
            self.keys.insert(p.id.clone(), String::new());
            self.config.providers.push(p);
        }
        if let Some((id, op)) = action {
            self.start(id, op, ui.ctx());
        }
        self.show_error(ui);
    }
    pub fn translation(&mut self, ui: &mut egui::Ui) {
        super::app::card(ui, |ui| {
            configure_form(ui);
            let field_width = (ui.available_width() - 96.0).clamp(120.0, 280.0);
            egui::Grid::new("translation-settings")
                .num_columns(2)
                .min_row_height(FORM_HEIGHT)
                .spacing([20.0, 12.0])
                .show(ui, |ui| {
                    provider_label(ui, "翻译模型");
                    let selected = self
                        .config
                        .providers
                        .iter()
                        .find(|p| p.id == self.config.provider)
                        .filter(|p| p.models.contains(&self.config.model))
                        .map(|p| format!("{} / {}", p.name, self.config.model))
                        .unwrap_or_else(|| "请选择模型".into());
                    egui::ComboBox::from_id_salt("translation-model")
                        .width(field_width)
                        .truncate()
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            for p in &self.config.providers {
                                for model in &p.models {
                                    if ui
                                        .selectable_label(
                                            p.id == self.config.provider
                                                && *model == self.config.model,
                                            format!("{} / {model}", p.name),
                                        )
                                        .clicked()
                                    {
                                        self.config.provider = p.id.clone();
                                        self.config.model = model.clone();
                                        self.config.reasoning = "default".into();
                                    }
                                }
                            }
                        });
                    ui.end_row();
                    if self
                        .config
                        .selected()
                        .is_ok_and(|p| p.kind == ApiKind::Compatible)
                        && supports_reasoning(&self.config.model)
                    {
                        provider_label(ui, "思考等级");
                        egui::ComboBox::from_id_salt("reasoning")
                            .width(field_width)
                            .truncate()
                            .selected_text(reasoning_label(&self.config.reasoning))
                            .show_ui(ui, |ui| {
                                for (value, label) in [
                                    ("default", "模型默认"),
                                    ("low", "低"),
                                    ("medium", "中"),
                                    ("high", "高"),
                                ] {
                                    ui.selectable_value(
                                        &mut self.config.reasoning,
                                        value.into(),
                                        label,
                                    );
                                }
                            });
                        ui.end_row();
                    }
                    provider_label(ui, "翻译为");
                    egui::ComboBox::from_id_salt("translation-target")
                        .width(field_width)
                        .truncate()
                        .selected_text(&self.config.target)
                        .show_ui(ui, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.languages)
                                    .hint_text("搜索语言"),
                            );
                            for language in [
                                "English",
                                "简体中文",
                                "繁體中文",
                                "日本語",
                                "한국어",
                                "Français",
                                "Deutsch",
                                "Español",
                                "Português",
                                "Italiano",
                                "Русский",
                                "العربية",
                                "हिन्दी",
                                "ไทย",
                                "Tiếng Việt",
                                "Bahasa Indonesia",
                            ] {
                                if language
                                    .to_lowercase()
                                    .contains(&self.languages.to_lowercase())
                                {
                                    ui.selectable_value(
                                        &mut self.config.target,
                                        language.into(),
                                        language,
                                    );
                                }
                            }
                        });
                    ui.end_row();
                    provider_label(ui, "结果处理");
                    egui::ComboBox::from_id_salt("translation-output")
                        .width(field_width)
                        .truncate()
                        .selected_text(if self.config.preview {
                            "预览后替换"
                        } else {
                            "自动替换全文"
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.config.preview, false, "自动替换全文");
                            ui.selectable_value(&mut self.config.preview, true, "预览后替换");
                        });
                    ui.end_row();
                });
            ui.add_space(8.0);
            egui::CollapsingHeader::new("补充要求").show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.config.instructions)
                        .desired_width(f32::INFINITY)
                        .desired_rows(3)
                        .hint_text("语气、专业术语或其他翻译要求"),
                );
                ui.horizontal(|ui| {
                    ui.label("超时（秒）");
                    ui.add(egui::DragValue::new(&mut self.config.timeout_seconds).range(5..=180));
                    if ui.button("恢复默认").clicked() {
                        self.config.instructions.clear();
                        self.config.timeout_seconds = 60;
                        self.config.reasoning = "default".into();
                    }
                });
            });
        });
        ui.add_space(12.0);
        egui::CollapsingHeader::new("翻译测试").show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.test_text)
                    .desired_width(f32::INFINITY)
                    .desired_rows(3)
                    .hint_text("输入要试译的文字"),
            );
            let loading = self
                .job
                .as_ref()
                .is_some_and(|j| j.id == "translation-test");
            if ui
                .add_enabled(
                    !loading && !self.test_text.trim().is_empty() && self.config.selected().is_ok(),
                    egui::Button::new("翻译"),
                )
                .clicked()
            {
                self.test_output.clear();
                self.start(
                    "translation-test".into(),
                    Operation::Translate {
                        text: self.test_text.clone(),
                    },
                    ui.ctx(),
                );
            }
            if loading {
                ui.spinner();
            }
            if let Some(message) = self.messages.get("translation-test") {
                ui.label(message);
            }
            if !self.test_output.is_empty() {
                ui.label(&self.test_output);
                if ui.button("复制译文").clicked() {
                    ui.ctx().copy_text(self.test_output.clone());
                }
            }
        });
        self.show_error(ui);
    }
    pub fn shortcuts_page(&mut self, ui: &mut egui::Ui) {
        if let Some(mode) = self.capture {
            let mods = ui.input(|i| i.modifiers);
            let bits = u8::from(mods.ctrl)
                | (u8::from(mods.alt) * 2)
                | (u8::from(mods.shift) * 4)
                | (u8::from(win_held()) * 8);
            let keys = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Key {
                            key,
                            pressed: true,
                            repeat: false,
                            ..
                        } => Some(*key),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            });
            let mut captured = None;
            for key in keys {
                if key == egui::Key::Escape {
                    self.capture = None;
                    self.tap = 0;
                    break;
                }
                if let Some(vk) = virtual_key(key) {
                    captured = Some(Shortcut {
                        vk,
                        modifiers: bits,
                    });
                    break;
                }
            }
            if mode && captured.is_none() {
                if bits == 4 {
                    self.tap = 0x10;
                } else if bits == 1 {
                    self.tap = 0x11;
                } else if bits != 0 {
                    self.tap = 0;
                }
                if bits == 0 && self.tap != 0 {
                    captured = Some(Shortcut {
                        vk: self.tap,
                        modifiers: 0,
                    });
                    self.tap = 0;
                }
            }
            if let Some(binding) = captured {
                self.tap = 0;
                let mut candidate = self.shortcuts;
                if mode {
                    candidate.mode = binding;
                } else {
                    candidate.translate = binding;
                }
                match candidate.validate().and_then(|_| {
                    retype_ai::secrets::save_shortcuts(candidate)
                        .map_err(|_| "无法保存快捷键".into())
                }) {
                    Ok(()) => {
                        self.shortcuts = candidate;
                        self.capture = None;
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        super::app::card(ui, |ui| {
            configure_form(ui);
            let field_width = (ui.available_width() - 234.0).clamp(120.0, 280.0);
            egui::Grid::new("shortcuts-settings")
                .num_columns(2)
                .min_row_height(FORM_HEIGHT)
                .spacing([20.0, 12.0])
                .show(ui, |ui| {
                    for (mode, label, binding) in [
                        (true, "切换中英文", self.shortcuts.mode),
                        (false, "翻译输入框全文", self.shortcuts.translate),
                    ] {
                        form_label(ui, label, 144.0);
                        ui.horizontal(|ui| {
                            let capturing = self.capture == Some(mode);
                            let text = if capturing {
                                let mut prefix =
                                    ui.input(|i| ui.ctx().format_modifiers(i.modifiers));
                                if win_held() {
                                    if !prefix.is_empty() {
                                        prefix.push_str(" + ");
                                    }
                                    prefix.push_str("Win");
                                }
                                if prefix.is_empty() {
                                    "请按快捷键…".into()
                                } else {
                                    format!("{prefix} + …")
                                }
                            } else {
                                binding.label()
                            };
                            let button =
                                egui::Button::new(RichText::new(text).color(if capturing {
                                    egui::Color32::WHITE
                                } else {
                                    super::app::INK
                                }))
                                .fill(if capturing {
                                    super::app::ACCENT
                                } else {
                                    egui::Color32::WHITE
                                });
                            if ui.add_sized([field_width, FORM_HEIGHT], button).clicked() {
                                self.capture = if capturing { None } else { Some(mode) };
                                self.tap = 0;
                            }
                            if ui.button("清除").clicked() {
                                let mut candidate = self.shortcuts;
                                if mode {
                                    candidate.mode = Shortcut::DISABLED;
                                } else {
                                    candidate.translate = Shortcut::DISABLED;
                                }
                                if retype_ai::secrets::save_shortcuts(candidate).is_ok() {
                                    self.shortcuts = candidate;
                                    self.capture = None;
                                }
                            }
                        });
                        ui.end_row();
                    }
                });
        });
        ui.add_space(12.0);
        if ui.button("恢复默认").clicked() {
            match retype_ai::secrets::save_shortcuts(Shortcuts::default()) {
                Ok(()) => {
                    self.shortcuts = Shortcuts::default();
                    self.capture = None;
                }
                Err(_) => self.error = Some("无法保存快捷键".into()),
            }
        }
        self.show_error(ui);
    }
    fn show_error(&self, ui: &mut egui::Ui) {
        if let Some(e) = &self.error {
            ui.colored_label(egui::Color32::DARK_RED, e);
        }
    }
}
impl Drop for AiPages {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.cancelled.store(true, Ordering::Relaxed);
        }
        let _ = self.flush();
    }
}
#[allow(unsafe_code)]
fn win_held() -> bool {
    // SAFETY: read-only current message keyboard state, no hooks or global interception.
    unsafe {
        windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x5b) < 0
            || windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x5c) < 0
    }
}
const FORM_HEIGHT: f32 = 36.0;
fn configure_form(ui: &mut egui::Ui) {
    let text_height = ui
        .text_style_height(&egui::TextStyle::Button)
        .max(ui.spacing().icon_width);
    ui.spacing_mut().interact_size.y = FORM_HEIGHT;
    ui.spacing_mut().button_padding = egui::vec2(10.0, (FORM_HEIGHT - text_height).max(0.0) / 2.0);
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 12.0);
}
fn provider_label(ui: &mut egui::Ui, text: &str) {
    form_label(ui, text, 76.0);
}
fn form_label(ui: &mut egui::Ui, text: &str, width: f32) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, FORM_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend));
        },
    );
}
fn provider_text(ui: &mut egui::Ui, value: &mut String, width: f32, password: bool) {
    ui.add(
        egui::TextEdit::singleline(value)
            .font(egui::TextStyle::Body)
            .desired_width(width)
            .min_size(egui::vec2(width, FORM_HEIGHT))
            .margin(egui::Margin::symmetric(10, 0))
            .vertical_align(egui::Align::Center)
            .password(password),
    );
}
fn kind_name(kind: ApiKind) -> &'static str {
    match kind {
        ApiKind::Compatible => "OpenAI 兼容",
        ApiKind::Anthropic => "Anthropic",
        ApiKind::Gemini => "Gemini",
        ApiKind::Ollama => "Ollama",
    }
}
fn reasoning_label(value: &str) -> &str {
    match value {
        "low" => "低",
        "medium" => "中",
        "high" => "高",
        _ => "模型默认",
    }
}
fn supports_reasoning(model: &str) -> bool {
    model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("gpt-5")
}

fn virtual_key(key: egui::Key) -> Option<u16> {
    if key == egui::Key::Space {
        return Some(0x20);
    }
    let name = format!("{key:?}");
    if name.len() == 1 {
        let c = name.as_bytes()[0];
        if c.is_ascii_uppercase() {
            return Some(c as u16);
        }
    }
    if let Some(number) = name
        .strip_prefix("Num")
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|n| *n <= 9)
    {
        return Some(0x30 + number);
    }
    name.strip_prefix('F')
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|n| (1..=24).contains(n))
        .map(|n| 0x6f + n)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forms_fit_normal_and_minimum_content_widths() {
        for width in [420.0, 620.0] {
            for page in 0..3 {
                let provider = Provider {
                    id: "headless-fixture".into(),
                    name: "示例服务".into(),
                    base_url: "https://example.invalid/v1".into(),
                    models: vec!["example-model".into()],
                    ..Default::default()
                };
                let config = Config {
                    provider: provider.id.clone(),
                    model: "example-model".into(),
                    providers: vec![provider],
                    ..Default::default()
                };
                let keys = HashMap::from([("headless-fixture".into(), String::new())]);
                let mut app = AiPages::from_config(config, keys, Shortcuts::default(), None);
                let ctx = egui::Context::default();
                assert!(super::super::app::chinese_fonts(&ctx).is_ok());
                ctx.set_theme(egui::Theme::Light);
                ctx.style_mut_of(egui::Theme::Light, |s| {
                    s.text_styles
                        .insert(egui::TextStyle::Body, egui::FontId::proportional(16.0));
                    s.text_styles
                        .insert(egui::TextStyle::Button, egui::FontId::proportional(16.0));
                    s.spacing.item_spacing = egui::vec2(10.0, 12.0);
                });
                let mut right = 0.0;
                for _ in 0..3 {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 900.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            match page {
                                0 => app.providers(ui),
                                1 => app.translation(ui),
                                _ => app.shortcuts_page(ui),
                            }
                            right = ui.min_rect().right();
                        },
                    );
                    assert!(!output.shapes.is_empty());
                    output.textures_delta.clear();
                }
                assert!(
                    right <= width + 1.0,
                    "page {page} extends to {right} at width {width}"
                );
                assert!(
                    app.job.is_none(),
                    "layout tests must not contact a provider"
                );
                assert_eq!(
                    app.config, app.saved,
                    "layout tests must not alter stored settings"
                );
                assert_eq!(app.keys, app.saved_keys);
            }
        }
    }
    #[test]
    fn recording_maps_letters_numbers_and_function_keys() {
        assert_eq!(virtual_key(egui::Key::A), Some(0x41));
        assert_eq!(virtual_key(egui::Key::Num9), Some(0x39));
        assert_eq!(virtual_key(egui::Key::F12), Some(0x7b));
        assert_eq!(virtual_key(egui::Key::Escape), None);
    }
}
