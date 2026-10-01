use crate::{
    backend::{self, Event, Task},
    instance,
    statistics::{self, Period},
};
use eframe::egui::{self, Color32, FontFamily, RichText, Stroke, Vec2};
use raw_window_handle::HasWindowHandle;
use retype_types::PinyinScheme;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, Sender},
    Arc,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

const INK: Color32 = Color32::from_rgb(20, 43, 59);
const ACCENT: Color32 = Color32::from_rgb(19, 143, 150);
const MUTED: Color32 = Color32::from_rgb(101, 119, 129);
const BACKGROUND: Color32 = Color32::from_rgb(246, 249, 250);
const BORDER: Color32 = Color32::from_rgb(227, 233, 236);
const PREFERENCES: &str = "Software\\retype";

type AppResult<T> = backend::Result<T>;

#[allow(unsafe_code)]
fn tick_count() -> u32 {
    // SAFETY: GetTickCount has no arguments or memory preconditions.
    unsafe { windows::Win32::System::SystemInformation::GetTickCount() }
}

struct Options {
    renderer: eframe::Renderer,
    timing_file: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    exit_after: Option<Duration>,
    opened_at: u32,
    updates: bool,
    page: Page,
    idle_exit: Duration,
    hide_after: Option<Duration>,
}

impl Options {
    fn parse() -> AppResult<Self> {
        let mut options = Self {
            renderer: eframe::Renderer::Glow,
            timing_file: None,
            screenshot: None,
            exit_after: None,
            opened_at: tick_count(),
            updates: false,
            page: Page::Input,
            idle_exit: Duration::from_secs(600),
            hide_after: None,
        };
        for argument in std::env::args().skip(1) {
            if let Some(value) = argument.strip_prefix("--renderer=") {
                options.renderer = match value {
                    "glow" => eframe::Renderer::Glow,
                    #[cfg(feature = "wgpu")]
                    "wgpu" => eframe::Renderer::Wgpu,
                    _ => return Err("renderer must be glow or wgpu".into()),
                };
            } else if let Some(value) = argument.strip_prefix("--timing-file=") {
                options.timing_file = Some(value.into());
            } else if let Some(value) = argument.strip_prefix("--screenshot=") {
                options.screenshot = Some(value.into());
            } else if let Some(value) = argument.strip_prefix("--exit-after-ms=") {
                options.exit_after = Some(Duration::from_millis(value.parse()?));
            } else if let Some(value) = argument.strip_prefix("--opened-at=") {
                options.opened_at = value.parse()?;
            } else if let Some(value) = argument.strip_prefix("--idle-exit-ms=") {
                options.idle_exit = Duration::from_millis(value.parse()?);
            } else if let Some(value) = argument.strip_prefix("--hide-after-ms=") {
                options.hide_after = Some(Duration::from_millis(value.parse()?));
            } else if argument == "--updates" {
                options.updates = true;
                options.page = Page::About;
            } else if let Some(value) = argument.strip_prefix("--page=") {
                options.page = match value {
                    "input" => Page::Input,
                    "dictionary" => Page::Dictionary,
                    "statistics" => Page::Statistics,
                    "about" => Page::About,
                    _ => return Err("Unknown settings page".into()),
                };
            } else if argument.starts_with("--open-reason=") {
                // Launcher diagnostic, accepted for compatibility with old TIPs.
            } else {
                return Err(format!("unknown argument: {argument}").into());
            }
        }
        Ok(options)
    }
}

#[cfg(test)]
fn read_scheme_at(key: &str) -> AppResult<PinyinScheme> {
    Ok(if backend::dword_at(key, "PinyinScheme")? == Some(1) {
        PinyinScheme::Flypy
    } else {
        PinyinScheme::Full
    })
}
fn write_scheme_at(key: &str, scheme: PinyinScheme) -> AppResult<()> {
    backend::set_at(
        key,
        "PinyinScheme",
        u32::from(scheme == PinyinScheme::Flypy),
    )
}
fn logo() -> AppResult<egui::ColorImage> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(include_bytes!(
        "../../settings/assets/logo.png"
    )))
    .read_info()?;
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels)?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err("logo must be an 8-bit RGBA PNG".into());
    }
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [info.width as usize, info.height as usize],
        &pixels[..info.buffer_size()],
    ))
}

fn chinese_fonts(ctx: &egui::Context) -> AppResult<String> {
    let root = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    for name in ["msyh.ttc", "msjh.ttc", "simhei.ttf", "simsun.ttc"] {
        let Ok(bytes) = std::fs::read(root.join("Fonts").join(name)) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "windows-chinese".into(),
            egui::FontData::from_owned(bytes).into(),
        );
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, "windows-chinese".into());
        }
        ctx.set_fonts(fonts);
        return Ok(name.into());
    }
    Err("No Windows Chinese font is available".into())
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Page {
    Input,
    Dictionary,
    Statistics,
    About,
}

struct SettingsApp {
    options: Options,
    started: Instant,
    scheme: PinyinScheme,
    page: Page,
    preferences: backend::Preferences,
    packs: Vec<backend::Pack>,
    installed_packs: Vec<bool>,
    busy_pack: Option<usize>,
    pack_progress: f32,
    pack_can_cancel: bool,
    cancel_download: Arc<AtomicBool>,
    statistics: Option<statistics::Snapshot>,
    period: Period,
    statistics_pending: bool,
    statistics_refresh: Instant,
    confirm_clear: bool,
    tasks: Sender<Task>,
    events: Receiver<Event>,
    offer: Option<backend::Offer>,
    update_message: String,
    updating: bool,
    installing: bool,
    hidden_since: Option<Instant>,
    instance: instance::Window,
    error: Option<String>,
    logo: egui::TextureHandle,
    font: String,
    font_ms: f64,
    initialization_ms: f64,
    first_ui_ms: Option<f64>,
    reported: bool,
    screenshot_requested: bool,
}

impl SettingsApp {
    fn select_page(&mut self, page: Page) {
        self.page = page;
        match page {
            Page::Dictionary => {
                let _ = self.tasks.send(Task::Packs);
            }
            Page::Statistics if !self.statistics_pending => {
                self.statistics_pending = true;
                let _ = self.tasks.send(Task::Statistics);
            }
            _ => {}
        }
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.hidden_since = Some(Instant::now());
        self.instance.idle_timer(self.options.idle_exit);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        ctx.request_repaint_after(Duration::from_secs(600));
    }

    fn lifecycle(&mut self, ctx: &egui::Context) {
        let requests = std::mem::take(
            &mut *self
                .instance
                .requests
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        for (opened_at, updates) in requests {
            self.hidden_since = None;
            self.options.opened_at = opened_at;
            self.reported = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            if let Ok(preferences) = backend::preferences() {
                self.scheme = preferences.scheme;
                self.preferences = preferences;
            }
            if updates {
                self.select_page(Page::About);
                self.check_update();
            } else {
                self.select_page(self.page);
            }
        }
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Packs(installed) => self.installed_packs = installed,
                Event::PackProgress(progress) => self.pack_progress = progress,
                Event::PackDone(result) => {
                    self.busy_pack = None;
                    if let Err(error) = result {
                        if !self.cancel_download.load(Ordering::Relaxed) {
                            self.error = Some(error);
                        }
                    }
                    if let Ok(prefs) = backend::preferences() {
                        self.preferences = prefs;
                    }
                    let _ = self.tasks.send(Task::Packs);
                }
                Event::Statistics(snapshot) => {
                    self.statistics = Some(snapshot);
                    self.statistics_pending = false;
                    self.statistics_refresh = Instant::now();
                }
                Event::Checked(result) => {
                    self.updating = false;
                    match result {
                        Ok(offer) => {
                            self.update_message = if offer.available {
                                if offer.installable {
                                    format!(
                                        "发现新版本 {}。",
                                        offer.version.as_deref().unwrap_or("")
                                    )
                                } else {
                                    "发现新版本，但缺少安装包或校验文件。".into()
                                }
                            } else {
                                "已是最新版本。".into()
                            };
                            self.offer = Some(offer);
                        }
                        Err(error) => self.update_message = format!("检查失败：{error}"),
                    }
                }
                Event::UpdateStage(message) => self.update_message = message,
                Event::Installed(result) => {
                    self.installing = false;
                    self.update_message = match result {
                        Ok(message) => {
                            self.offer = None;
                            message
                        }
                        Err(error) => format!("更新未完成：{error}"),
                    };
                    if let Ok(prefs) = backend::preferences() {
                        self.preferences = prefs;
                    }
                }
                Event::Skipped(result) => {
                    self.update_message = match result {
                        Ok(()) => "已跳过此版本的自动提醒。".into(),
                        Err(error) => format!("保存失败：{error}"),
                    }
                }
                Event::Error(error) => {
                    self.error = Some(error);
                    self.statistics_pending = false;
                    self.statistics_refresh = Instant::now();
                }
            }
        }
        if let Some(hidden) = self.hidden_since {
            if hidden.elapsed() >= Duration::from_secs(600)
                && self.busy_pack.is_none()
                && !self.updating
                && !self.installing
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        } else if self.page == Page::Statistics {
            if !self.statistics_pending
                && self.statistics_refresh.elapsed() >= Duration::from_secs(5)
            {
                self.statistics_pending = true;
                let _ = self.tasks.send(Task::Statistics);
            }
            ctx.request_repaint_after(Duration::from_secs(5));
        }
    }

    fn save(&mut self, name: &str, value: bool) -> bool {
        match backend::set(name, u32::from(value)) {
            Ok(()) => {
                self.error = None;
                true
            }
            Err(error) => {
                self.error = Some(format!("保存设置失败：{error}"));
                false
            }
        }
    }

    fn check_update(&mut self) {
        if self.updating || self.installing {
            return;
        }
        self.updating = true;
        self.update_message = "正在检查更新…".into();
        let _ = self.tasks.send(Task::Check);
    }

    fn open(&mut self, destination: &str) {
        if let Err(error) = backend::open(destination) {
            self.error = Some(error.to_string());
        }
    }

    fn dictionary_page(&mut self, ui: &mut egui::Ui) {
        heading(ui, "词库", "按需下载扩展词库，补充专业名词、人名与地名。");
        card(ui, |ui| {
            ui.label(RichText::new("基础词库已随输入法安装").strong());
            ui.label(
                RichText::new("扩展词库来自万象；下载后仅保存在本机，可随时开启或停用。")
                    .size(13.0)
                    .color(MUTED),
            );
        });
        ui.add_space(10.0);
        if self.installed_packs.is_empty() {
            ui.spinner();
            ui.label("正在读取词库…");
        }
        for index in 0..self.packs.len() {
            let pack = self.packs[index].clone();
            let installed = self.installed_packs.get(index).copied().unwrap_or(false);
            let enabled = installed && self.preferences.pack_mask & (1 << index) != 0;
            let busy = self.busy_pack == Some(index);
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    let info_width = (ui.available_width() - 140.0).max(120.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(info_width, 54.0),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            ui.set_width(info_width);
                            ui.label(RichText::new(&pack.title).strong());
                            ui.label(
                                RichText::new(format!(
                                    "{} · {:.1} MB",
                                    pack.description,
                                    pack.bytes as f32 / 1048576.0
                                ))
                                .size(13.0)
                                .color(MUTED),
                            );
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if busy && self.pack_can_cancel {
                            if !self.cancel_download.load(Ordering::Relaxed)
                                && ui.button("取消").clicked()
                            {
                                self.cancel_download.store(true, Ordering::Relaxed);
                            }
                        } else if !busy {
                            let mut next_enabled = enabled;
                            let changed = ui
                                .add_enabled_ui(
                                    self.busy_pack.is_none() && !self.installed_packs.is_empty(),
                                    |ui| {
                                        toggle(
                                            ui,
                                            &mut next_enabled,
                                            &format!("启用{}", pack.title),
                                        )
                                    },
                                )
                                .inner;
                            if changed {
                                self.busy_pack = Some(index);
                                self.pack_progress = 0.0;
                                self.pack_can_cancel = !enabled && !installed;
                                self.cancel_download = Arc::new(AtomicBool::new(false));
                                self.error = None;
                                let _ = self.tasks.send(Task::SetPack(
                                    index,
                                    next_enabled,
                                    Arc::clone(&self.cancel_download),
                                ));
                            }
                        }
                    });
                });
                if busy {
                    ui.add(egui::ProgressBar::new(self.pack_progress).show_percentage());
                    ui.label(
                        RichText::new(if self.cancel_download.load(Ordering::Relaxed) {
                            "正在取消…"
                        } else if self.pack_progress >= 1.0 {
                            "正在校验和转换词库…"
                        } else {
                            "正在下载…"
                        })
                        .size(12.0)
                        .color(MUTED),
                    );
                } else {
                    ui.label(
                        RichText::new(if enabled {
                            "已启用"
                        } else if installed {
                            "已下载 · 未启用"
                        } else {
                            "未下载"
                        })
                        .size(12.0)
                        .color(if enabled { ACCENT } else { MUTED }),
                    );
                }
            });
            ui.add_space(8.0);
        }
        ui.hyperlink_to("词库来源与许可", "https://github.com/amzxyz/rime-wanxiang");
    }

    fn statistics_page(&mut self, ui: &mut egui::Ui) {
        use chrono::Datelike;
        heading(ui, "统计", "中文与英文分别统计，只记录数量和有效输入时间。");
        let Some(snapshot) = self.statistics.clone() else {
            ui.spinner();
            ui.label("正在读取统计…");
            return;
        };
        ui.columns(2, |columns| {
            for (column, title, counts) in [
                (0, "今日输入", snapshot.today),
                (1, "累计输入", snapshot.total),
            ] {
                card(&mut columns[column], |ui| {
                    ui.label(title);
                    ui.label(
                        RichText::new(counts.total().to_string())
                            .size(30.0)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(format!(
                            "中文 {} 字 · 英文 {} 字符",
                            counts.chinese, counts.english
                        ))
                        .size(12.0)
                        .color(MUTED),
                    );
                });
            }
        });
        ui.add_space(14.0);
        card(ui, |ui| {
            ui.label(RichText::new("近期速度").strong());
            ui.label(
                RichText::new("最近 5 分钟；停止输入 30 秒后不显示速度。")
                    .size(12.0)
                    .color(MUTED),
            );
            ui.columns(2, |columns| {
                speed_value(&mut columns[0], "中文", "字/分钟", snapshot.chinese_speed);
                speed_value(&mut columns[1], "英文", "字符/分钟", snapshot.english_speed);
            });
        });
        ui.add_space(14.0);
        card(ui, |ui| {
            ui.label(RichText::new("平均速度").strong());
            ui.label(
                RichText::new("本期截至当前；与上一完整周期比较")
                    .size(12.0)
                    .color(MUTED),
            );
            ui.horizontal(|ui| {
                for period in [Period::Day, Period::Week, Period::Month, Period::Year] {
                    ui.selectable_value(&mut self.period, period, period.label());
                }
            });
            let history = snapshot.history(chrono::Local::now().date_naive(), self.period);
            if history.len() >= 2 {
                let current = history[history.len() - 1].1;
                let previous = history[history.len() - 2].1;
                ui.columns(2, |columns| {
                    for (index, language, unit, speed, prior, count, active) in [
                        (
                            0,
                            "中文",
                            "字/分钟",
                            current.chinese_speed(),
                            previous.chinese_speed(),
                            current.chinese,
                            current.chinese_ms,
                        ),
                        (
                            1,
                            "英文",
                            "字符/分钟",
                            current.english_speed(),
                            previous.english_speed(),
                            current.english,
                            current.english_ms,
                        ),
                    ] {
                        speed_value(&mut columns[index], language, unit, speed);
                        let change = match (speed, prior) {
                            (Some(now), Some(before)) if before > 0 => format!(
                                "较上期 {:+.0}%",
                                (now as f64 - before as f64) * 100.0 / before as f64
                            ),
                            (Some(_), _) => "上期无可比数据".into(),
                            _ => "样本不足".into(),
                        };
                        columns[index].label(RichText::new(change).size(12.0).color(MUTED));
                        columns[index].label(
                            RichText::new(format!(
                                "{count} {} · 有效 {}",
                                if index == 0 { "字" } else { "字符" },
                                active_duration(active)
                            ))
                            .size(11.0)
                            .color(MUTED),
                        );
                    }
                });
            }
            let data: Vec<_> = history
                .iter()
                .map(|(date, counts)| {
                    (
                        match self.period {
                            Period::Day | Period::Week => {
                                format!("{}/{}", date.month(), date.day())
                            }
                            Period::Month => format!("{}月", date.month()),
                            Period::Year => date.year().to_string(),
                        },
                        counts.chinese_speed(),
                        counts.english_speed(),
                    )
                })
                .collect();
            chart(ui, &data, false);
            ui.label(
                RichText::new("均速 = 上屏字符总数 ÷ 有效输入时间；样本不足时不显示速度。")
                    .size(12.0)
                    .color(MUTED),
            );
        });
        ui.add_space(14.0);
        card(ui, |ui| {
            ui.label(RichText::new("最近 7 天输入量").strong());
            let today = chrono::Local::now().date_naive();
            let data: Vec<_> = (0..7)
                .map(|i| {
                    let date = today - chrono::Duration::days(6 - i);
                    let counts = snapshot.days.get(&date).copied().unwrap_or_default();
                    (
                        format!("{}/{}", date.month(), date.day()),
                        Some(counts.chinese),
                        Some(counts.english),
                    )
                })
                .collect();
            chart(ui, &data, true);
        });
        ui.add_space(14.0);
        card(ui, |ui| {
            let mut enabled = self.preferences.statistics_enabled;
            if switch_row(ui, &mut enabled, "记录输入统计")
                && self.save("StatisticsEnabled", enabled)
            {
                self.preferences.statistics_enabled = enabled;
                self.statistics_pending = true;
                let _ = self.tasks.send(Task::Statistics);
            }
            ui.label(
                RichText::new("中文按上屏汉字数统计，英文按字母数统计；不保存输入内容。")
                    .size(12.0)
                    .color(MUTED),
            );
            if ui.button("清空统计数据").clicked() {
                self.confirm_clear = true;
            }
        });
        if self.confirm_clear {
            egui::Modal::new(egui::Id::new("clear-statistics")).show(ui.ctx(), |ui| {
                ui.heading("清空统计数据？");
                ui.label("今日和历史的输入数量及速度记录会清空。");
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        self.confirm_clear = false;
                    }
                    if ui.button("清空").clicked() {
                        self.confirm_clear = false;
                        self.statistics_pending = true;
                        let _ = self.tasks.send(Task::ClearStatistics);
                    }
                });
            });
        }
    }

    fn about_page(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(6.0);
            ui.add(egui::Image::new(&self.logo).fit_to_exact_size(Vec2::splat(86.0)));
            ui.add_space(16.0);
            ui.label(RichText::new("retype 输入法").size(28.0).strong());
            ui.label(RichText::new(&self.preferences.version).color(MUTED));
            ui.add_space(24.0);
        });
        card(ui, |ui| {
            let mut enabled = self.preferences.auto_check;
            if switch_row(ui, &mut enabled, "每天自动检查并提醒") && self.save("AutoCheck", enabled)
            {
                self.preferences.auto_check = enabled;
            }
            ui.label(
                RichText::new("发现新版本时提醒，不会自动下载安装。")
                    .size(12.0)
                    .color(MUTED),
            );
            ui.separator();
            ui.label(RichText::new("更新").strong());
            ui.label(RichText::new(&self.update_message).size(14.0).color(MUTED));
            if self.updating || self.installing {
                ui.spinner();
            }
            let offer = self.offer.clone();
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        !self.updating && !self.installing,
                        egui::Button::new("检查更新"),
                    )
                    .clicked()
                {
                    self.check_update();
                }
                if let Some(offer) = offer {
                    if offer.available
                        && offer.installable
                        && ui
                            .add_enabled(
                                !self.installing,
                                egui::Button::new("下载并安装").fill(ACCENT),
                            )
                            .clicked()
                    {
                        if let Some(version) = &offer.version {
                            self.installing = true;
                            self.update_message = "正在准备下载…".into();
                            let _ = self.tasks.send(Task::Install(version.clone()));
                        }
                    }
                    if let Some(url) = offer.url.filter(|url| {
                        url.starts_with("https://github.com/L-Chris/retype/releases/")
                    }) {
                        if ui.button("版本说明").clicked() {
                            self.open(&url);
                        }
                    }
                    if offer.available
                        && ui
                            .add_enabled(!self.installing, egui::Button::new("跳过此版本"))
                            .clicked()
                    {
                        if let Some(version) = &offer.version {
                            let _ = self.tasks.send(Task::Skip(version.clone()));
                        }
                    }
                }
            });
            ui.separator();
            if ui.button("反馈问题  ›").clicked() {
                self.open("https://github.com/L-Chris/retype/issues");
            }
        });
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("© 2026 retype · MIT License")
                    .size(12.0)
                    .color(MUTED),
            );
            ui.horizontal_wrapped(|ui| {
                for (title, file) in [("开源许可", "LICENSE"), ("第三方署名", "NOTICE.txt")]
                {
                    if ui.link(title).clicked() {
                        self.open(&self.preferences.directory.join(file).to_string_lossy());
                    }
                }
                ui.hyperlink_to("项目主页", "https://github.com/L-Chris/retype");
            });
        });
    }
    fn new(
        cc: &eframe::CreationContext<'_>,
        options: Options,
        started: Instant,
    ) -> AppResult<Self> {
        let font_started = Instant::now();
        let font = chinese_fonts(&cc.egui_ctx)?;
        let font_ms = font_started.elapsed().as_secs_f64() * 1000.0;
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::light();
        style.visuals.override_text_color = Some(INK);
        style.visuals.panel_fill = BACKGROUND;
        style.visuals.widgets.inactive.bg_fill = Color32::WHITE;
        style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
        style.visuals.widgets.inactive.weak_bg_fill = Color32::WHITE;
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(229, 247, 247);
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
        style.spacing.button_padding = Vec2::new(10.0, 7.0);
        style.visuals.selection.bg_fill = ACCENT;
        style.spacing.item_spacing = Vec2::new(10.0, 12.0);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(16.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(16.0));
        cc.egui_ctx.set_style(style);
        let image = logo()?;
        cc.egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::Icon(Some(std::sync::Arc::new(
                egui::IconData {
                    rgba: image.pixels.iter().flat_map(|c| c.to_array()).collect(),
                    width: image.size[0] as u32,
                    height: image.size[1] as u32,
                },
            ))));
        let preferences = backend::preferences()?;
        let scheme = preferences.scheme;
        let error = None;
        let page = options.page;
        let (tasks, events, busy) = backend::worker(cc.egui_ctx.clone());
        let handle = cc.window_handle()?.as_raw();
        let raw_window_handle::RawWindowHandle::Win32(handle) = handle else {
            return Err("Unsupported window handle".into());
        };
        let instance = instance::Window::create(
            windows::Win32::Foundation::HWND(handle.hwnd.get() as *mut _),
            cc.egui_ctx.clone(),
            busy,
        )?;
        if page == Page::Dictionary {
            let _ = tasks.send(Task::Packs);
        }
        if page == Page::Statistics {
            let _ = tasks.send(Task::Statistics);
        }
        if options.updates {
            let _ = tasks.send(Task::Check);
        }
        Ok(Self {
            initialization_ms: started.elapsed().as_secs_f64() * 1000.0,
            started,
            scheme,
            page,
            preferences,
            packs: backend::packs()?,
            installed_packs: Vec::new(),
            busy_pack: None,
            pack_progress: 0.0,
            pack_can_cancel: false,
            cancel_download: Arc::new(AtomicBool::new(false)),
            statistics: None,
            period: Period::Day,
            statistics_pending: page == Page::Statistics,
            statistics_refresh: Instant::now(),
            confirm_clear: false,
            tasks,
            events,
            offer: None,
            update_message: if options.updates {
                "正在检查更新…".into()
            } else {
                "检查是否有新版本。".into()
            },
            updating: options.updates,
            installing: false,
            hidden_since: None,
            instance,
            error,
            logo: cc
                .egui_ctx
                .load_texture("retype-logo", image, egui::TextureOptions::LINEAR),
            font,
            font_ms,
            first_ui_ms: None,
            reported: false,
            screenshot_requested: false,
            options,
        })
    }

    fn input_page(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("输入").size(28.0).strong());
        ui.add_space(8.0);
        ui.label(RichText::new("选择适合你的拼音输入方式。").color(MUTED));
        ui.add_space(24.0);
        ui.label(RichText::new("拼音方案").strong());
        ui.add_space(4.0);
        egui::Frame::new()
            .fill(Color32::WHITE)
            .stroke(Stroke::new(1.0_f32, BORDER))
            .corner_radius(14)
            .inner_margin(20)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (scheme, title, hint) in [
                    (PinyinScheme::Full, "全拼", "输入 nihao，空格上屏“你好”"),
                    (PinyinScheme::Flypy, "小鹤双拼", "输入 nihc，空格上屏“你好”"),
                ] {
                    let selected = self.scheme == scheme;
                    let response = ui.add_sized(
                        [ui.available_width(), 42.0],
                        egui::Button::new(RichText::new(title).strong().color(if selected {
                            ACCENT
                        } else {
                            INK
                        }))
                        .fill(if selected {
                            Color32::from_rgb(229, 247, 247)
                        } else {
                            Color32::WHITE
                        })
                        .stroke(Stroke::NONE)
                        .corner_radius(8),
                    );
                    if response.clicked() && !selected {
                        match write_scheme_at(PREFERENCES, scheme) {
                            Ok(()) => {
                                self.scheme = scheme;
                                self.error = None;
                            }
                            Err(error) => self.error = Some(format!("保存设置失败：{error}")),
                        }
                    }
                    ui.label(RichText::new(hint).size(13.0).color(MUTED));
                    if scheme == PinyinScheme::Full {
                        ui.separator();
                    }
                }
            });
        ui.add_space(12.0);
        ui.label(
            RichText::new("单按 Shift 切换中英文；组字时用 - 和 = 翻候选页。")
                .size(13.0)
                .color(MUTED),
        );
        if let Some(error) = &self.error {
            ui.colored_label(Color32::DARK_RED, error);
        }
    }

    fn write_timings(&mut self, rendered_previous_frame: bool) {
        if self.reported || !rendered_previous_frame {
            return;
        }
        self.reported = true;
        if let Ok(root) = backend::local_root() {
            use std::io::Write;
            let directory = root.join("logs");
            if std::fs::create_dir_all(&directory).is_ok() {
                let path = directory.join("settings-startup.log");
                if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() >= 256 * 1024) {
                    let previous = directory.join("settings-startup.log.previous");
                    let _ = std::fs::remove_file(&previous);
                    let _ = std::fs::rename(&path, &previous);
                }
                if let Ok(mut log) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let _ = writeln!(
                        log,
                        "{} pid={} event=egui.interactive open_ms={}",
                        chrono::Utc::now().to_rfc3339(),
                        std::process::id(),
                        tick_count().wrapping_sub(self.options.opened_at)
                    );
                }
            }
        }
        if let Some(path) = &self.options.timing_file {
            let report = serde_json::json!({
                "renderer": format!("{:?}", self.options.renderer),
                "font": self.font, "font_ms": self.font_ms,
                "initialization_ms": self.initialization_ms,
                "first_ui_ms": self.first_ui_ms,
                "next_frame_ms": self.started.elapsed().as_secs_f64() * 1000.0,
                "open_ms": tick_count().wrapping_sub(self.options.opened_at),
                "page": format!("{:?}",self.page),
            });
            if let Err(error) = std::fs::write(path, report.to_string()) {
                self.error = Some(format!("写入启动计时失败：{error}"));
            }
        }
    }
}

fn heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).size(28.0).strong());
    ui.add_space(8.0);
    ui.label(RichText::new(subtitle).size(13.0).color(MUTED));
    ui.add_space(24.0);
}
fn navigation(ui: &mut egui::Ui, page: Page, title: &str, selected: bool) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 48.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), title)
    });
    let painter = ui.painter();
    if selected || response.hovered() {
        painter.rect_filled(rect, 12, Color32::WHITE);
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            12,
            Stroke::new(1.0_f32, ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    let color = if selected { ACCENT } else { INK };
    let stroke = Stroke::new(1.7_f32, color);
    let origin = rect.left_center() + Vec2::new(26.0, 0.0);
    match page {
        Page::Input => {
            painter.rect_stroke(
                egui::Rect::from_center_size(origin, Vec2::new(18.0, 12.0)),
                2,
                stroke,
                egui::StrokeKind::Inside,
            );
            for row in 0..2 {
                for column in 0..4 {
                    painter.circle_filled(
                        origin + Vec2::new(-5.0 + column as f32 * 3.5, -2.5 + row as f32 * 3.0),
                        0.8,
                        color,
                    );
                }
            }
            painter.line_segment(
                [origin + Vec2::new(-4.0, 3.5), origin + Vec2::new(4.0, 3.5)],
                stroke,
            );
        }
        Page::Dictionary => {
            painter.rect_stroke(
                egui::Rect::from_center_size(origin, Vec2::new(17.0, 17.0)),
                2,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    origin + Vec2::new(-3.5, -7.0),
                    origin + Vec2::new(-3.5, 7.0),
                ],
                stroke,
            );
            painter.line_segment(
                [origin + Vec2::new(0.0, -3.0), origin + Vec2::new(5.0, -3.0)],
                stroke,
            );
            painter.line_segment(
                [origin + Vec2::new(0.0, 1.0), origin + Vec2::new(5.0, 1.0)],
                stroke,
            );
        }
        Page::Statistics => {
            for (i, height) in [7.0, 13.0, 18.0].into_iter().enumerate() {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        origin + Vec2::new(-8.0 + i as f32 * 6.0, 9.0 - height),
                        Vec2::new(4.0, height),
                    ),
                    1,
                    color,
                );
            }
        }
        Page::About => {
            painter.circle_stroke(origin, 8.5, stroke);
            painter.text(
                origin,
                egui::Align2::CENTER_CENTER,
                "i",
                egui::FontId::proportional(14.0),
                color,
            );
        }
    }
    painter.text(
        rect.left_center() + Vec2::new(52.0, 0.0),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(16.0),
        color,
    );
    response.clicked()
}
fn card(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(14)
        .inner_margin(20)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui);
        });
}
fn switch_row(ui: &mut egui::Ui, value: &mut bool, label: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = toggle(ui, value, label);
        });
    });
    changed
}
fn toggle(ui: &mut egui::Ui, value: &mut bool, label: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(42.0, 24.0), egui::Sense::click());
    if response.clicked() {
        *value = !*value;
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *value, label)
    });
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        12,
        if *value {
            ACCENT
        } else {
            Color32::from_rgb(194, 204, 209)
        },
    );
    painter.circle_filled(
        egui::pos2(
            if *value {
                rect.right() - 12.0
            } else {
                rect.left() + 12.0
            },
            rect.center().y,
        ),
        9.0,
        Color32::WHITE,
    );
    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(2.0),
            12,
            Stroke::new(1.0_f32, INK),
            egui::StrokeKind::Outside,
        );
    }
    response.clicked()
}
fn speed_value(ui: &mut egui::Ui, language: &str, unit: &str, speed: Option<u64>) {
    ui.label(RichText::new(language).size(13.0).color(MUTED));
    ui.label(
        RichText::new(speed.map(|v| v.to_string()).unwrap_or_else(|| "—".into()))
            .size(27.0)
            .strong(),
    );
    ui.label(RichText::new(unit).size(12.0).color(MUTED));
}
fn active_duration(ms: u64) -> String {
    if ms < 60_000 {
        format!("{:.0}秒", ms as f64 / 1000.0)
    } else if ms < 3_600_000 {
        format!("{:.1}分钟", ms as f64 / 60_000.0)
    } else {
        format!("{:.1}小时", ms as f64 / 3_600_000.0)
    }
}
fn chart(ui: &mut egui::Ui, data: &[(String, Option<u64>, Option<u64>)], stacked: bool) {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 150.0), egui::Sense::hover());
    let painter = ui.painter();
    let max = data
        .iter()
        .map(|(_, ch, en)| {
            if stacked {
                ch.unwrap_or(0) + en.unwrap_or(0)
            } else {
                ch.unwrap_or(0).max(en.unwrap_or(0))
            }
        })
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let step = rect.width() / data.len().max(1) as f32;
    let baseline = rect.bottom() - 24.0;
    for (i, (label, ch, en)) in data.iter().enumerate() {
        let x = rect.left() + step * (i as f32 + 0.5);
        let width = (step * 0.28).min(20.0);
        let h_ch = ch.unwrap_or(0) as f32 / max * 110.0;
        let h_en = en.unwrap_or(0) as f32 / max * 110.0;
        let blue = Color32::from_rgb(115, 185, 210);
        if stacked {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x - width, baseline - h_ch),
                    egui::pos2(x + width, baseline),
                ),
                2,
                ACCENT,
            );
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x - width, baseline - h_ch - h_en),
                    egui::pos2(x + width, baseline - h_ch),
                ),
                2,
                blue,
            );
        } else {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x - width - 1.0, baseline - h_ch),
                    egui::pos2(x - 1.0, baseline),
                ),
                2,
                ACCENT,
            );
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x + 1.0, baseline - h_en),
                    egui::pos2(x + width + 1.0, baseline),
                ),
                2,
                blue,
            );
        }
        painter.text(
            egui::pos2(x, baseline + 10.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.0),
            MUTED,
        );
        if response
            .hover_pos()
            .is_some_and(|p| p.x >= x - step / 2.0 && p.x < x + step / 2.0)
        {
            response.clone().on_hover_text(format!(
                "{label}\n中文 {}\n英文 {}",
                ch.map(|v| v.to_string())
                    .unwrap_or_else(|| "样本不足".into()),
                en.map(|v| v.to_string())
                    .unwrap_or_else(|| "样本不足".into())
            ));
        }
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("● 中文").size(12.0).color(ACCENT));
        ui.label(
            RichText::new("● 英文")
                .size(12.0)
                .color(Color32::from_rgb(115, 185, 210)),
        );
    });
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.lifecycle(ctx);
        self.write_timings(_frame.info().cpu_usage.is_some());
        egui::SidePanel::left("sidebar")
            .exact_width(238.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(221, 246, 248))
                    .inner_margin(20),
            )
            .show(ctx, |ui| {
                ui.add_space(12.0);
                let brand = ui.horizontal(|ui| {
                    ui.add(egui::Image::new(&self.logo).fit_to_exact_size(Vec2::splat(38.0)));
                    ui.label(RichText::new("retype 输入法").size(18.0).strong());
                });
                if brand.response.interact(egui::Sense::drag()).drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                ui.add_space(42.0);
                for (page, title) in [
                    (Page::Input, "输入"),
                    (Page::Dictionary, "词库"),
                    (Page::Statistics, "统计"),
                    (Page::About, "关于"),
                ] {
                    if navigation(ui, page, title, self.page == page) {
                        self.select_page(page);
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.label(RichText::new("简单、专注地输入").size(12.0).color(MUTED));
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(40))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.page {
                        Page::Input => self.input_page(ui),
                        Page::Dictionary => self.dictionary_page(ui),
                        Page::Statistics => self.statistics_page(ui),
                        Page::About => self.about_page(ui),
                    });
                if self.page != Page::Input {
                    if let Some(error) = &self.error {
                        ui.colored_label(Color32::DARK_RED, error);
                    }
                }
            });
        // Foreground areas keep controls visible above every scrollable page.
        // No separate title/header row consumes space in the window.
        let bounds = ctx.screen_rect();
        egui::Area::new(egui::Id::new("window-drag"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(238.0, 0.0))
            .show(ctx, |ui| {
                let response = ui.allocate_response(
                    Vec2::new((bounds.width() - 298.0).max(1.0), 28.0),
                    egui::Sense::drag(),
                );
                if response.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
            });
        egui::Area::new(egui::Id::new("window-close"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(bounds.right() - 48.0, 12.0))
            .show(ctx, |ui| {
                if ui
                    .add_sized(
                        [36.0, 32.0],
                        egui::Button::new(RichText::new("×").size(24.0).color(MUTED)).frame(false),
                    )
                    .on_hover_text("关闭")
                    .clicked()
                {
                    self.hide(ctx);
                }
            });
        if self.first_ui_ms.is_none() {
            self.first_ui_ms = Some(self.started.elapsed().as_secs_f64() * 1000.0);
            ctx.request_repaint();
        }
        if self.options.screenshot.is_some() && !self.screenshot_requested && self.reported {
            self.screenshot_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        if let Some(path) = &self.options.screenshot {
            for event in ctx.input(|input| input.events.clone()) {
                if let egui::Event::Screenshot { image, .. } = event {
                    let result = (|| -> AppResult<()> {
                        let mut encoder = png::Encoder::new(
                            std::fs::File::create(path)?,
                            image.size[0] as u32,
                            image.size[1] as u32,
                        );
                        encoder.set_color(png::ColorType::Rgba);
                        encoder.set_depth(png::BitDepth::Eight);
                        let pixels: Vec<u8> =
                            image.pixels.iter().flat_map(|c| c.to_array()).collect();
                        encoder.write_header()?.write_image_data(&pixels)?;
                        Ok(())
                    })();
                    if let Err(error) = result {
                        self.error = Some(format!("截图失败：{error}"));
                    }
                }
            }
        }
        if let Some(delay) = self.options.hide_after {
            if self.started.elapsed() >= delay {
                self.options.hide_after = None;
                self.hide(ctx);
            } else {
                ctx.request_repaint_after(delay.saturating_sub(self.started.elapsed()));
            }
        }
        if let Some(delay) = self.options.exit_after {
            if self.started.elapsed() >= delay {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint_after(delay.saturating_sub(self.started.elapsed()));
            }
        }
    }
}

pub fn run() -> AppResult<()> {
    let started = Instant::now();
    let options = Options::parse()?;
    let Some(_guard) = instance::acquire(options.updates, options.opened_at)? else {
        return Ok(());
    };
    let native = eframe::NativeOptions {
        renderer: options.renderer,
        centered: true,
        persist_window: false,
        viewport: egui::ViewportBuilder::default()
            .with_title("retype 设置")
            .with_decorations(false)
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "retype 设置",
        native,
        Box::new(move |cc| {
            SettingsApp::new(cc, options, started)
                .map(|app| Box::new(app) as Box<dyn eframe::App>)
                .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> {
                    error.to_string().into()
                })
        }),
    )
    .map_err(|error| error.to_string().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(unsafe_code)]
    fn scheme_round_trip_uses_the_shared_dword_format() -> AppResult<()> {
        // Exercise Windows persistence without changing the user's input mode.
        let key = format!(
            "Software\\retype\\Tests\\SettingsEgui-{}",
            std::process::id()
        );
        let result = (|| -> AppResult<()> {
            assert_eq!(read_scheme_at(&key)?, PinyinScheme::Full);
            write_scheme_at(&key, PinyinScheme::Flypy)?;
            assert_eq!(read_scheme_at(&key)?, PinyinScheme::Flypy);
            write_scheme_at(&key, PinyinScheme::Full)?;
            assert_eq!(read_scheme_at(&key)?, PinyinScheme::Full);
            Ok(())
        })();
        let key = backend::wide(&key);
        // SAFETY: Only the unique test key created above is removed.
        unsafe {
            windows::Win32::System::Registry::RegDeleteKeyW(
                windows::Win32::System::Registry::HKEY_CURRENT_USER,
                windows::core::PCWSTR(key.as_ptr()),
            )
            .ok()?;
        }
        result
    }
}
