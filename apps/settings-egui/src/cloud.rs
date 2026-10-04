use eframe::egui::{self, RichText};
use retype_sync::{
    config::{self, Config, PROVIDERS},
    runtime::{self, Command, Status},
    webdav::WebDav,
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
struct Job {
    config: Config,
    password: String,
    saving: bool,
    receiver: mpsc::Receiver<Result<String, String>>,
}
pub struct CloudPage {
    config: Config,
    saved: Config,
    password: String,
    saved_password: String,
    status: Status,
    error: Option<String>,
    job: Option<Job>,
    dirty: Option<Instant>,
    poll: Instant,
    resume_pending: bool,
}
impl CloudPage {
    pub fn new() -> Self {
        let loaded = config::root().and_then(|root| Config::load_at(&root));
        let mut error = loaded.as_ref().err().cloned();
        let config = loaded.unwrap_or_default();
        let password = runtime::password(&config).unwrap_or_else(|e| {
            error = Some(e);
            String::new()
        });
        let resume_pending = config.enabled && error.is_none();
        Self {
            saved: config.clone(),
            config,
            saved_password: password.clone(),
            password,
            status: Status::default(),
            error,
            job: None,
            dirty: None,
            poll: Instant::now() - Duration::from_secs(2),
            resume_pending,
        }
    }
    fn start(&mut self, ctx: &egui::Context, operation: u8, command: Option<Command>) {
        if self.job.is_some() {
            return;
        }
        let config = self.config.clone();
        let password = self.password.clone();
        let c = config.clone();
        let p = password.clone();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("retype-cloud-settings".into())
            .spawn(move || {
                let result = (|| {
                    if operation == 2 {
                        // Resume a saved enabled account after upgrade/restart without
                        // rewriting its credentials or confirming its initial merge.
                        runtime::command(&c, Command::Sync)?;
                        return Ok("同步中…".into());
                    }
                    if operation == 1 {
                        WebDav::new(&c, &p)?.test()?;
                        return Ok("连接成功，云盘支持同步读写".into());
                    }
                    runtime::save(&c, &p)?;
                    if let Some(command) = command {
                        runtime::command(&c, command)?;
                    }
                    Ok(if c.enabled {
                        "云同步已启用"
                    } else {
                        "云同步已关闭"
                    }
                    .into())
                })();
                let _ = tx.send(result);
                ctx.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.job = Some(Job {
                    config,
                    password,
                    saving: operation == 0,
                    receiver: rx,
                })
            }
            Err(_) => self.error = Some("无法创建云同步后台任务".into()),
        }
    }
    pub fn tick(&mut self, ctx: &egui::Context) -> bool {
        if self.resume_pending && self.job.is_none() {
            self.resume_pending = false;
            self.start(ctx, 2, None);
        }
        if let Some(result) = self.job.as_ref().and_then(|j| j.receiver.try_recv().ok()) {
            if let Some(job) = self.job.take() {
                match result {
                    Ok(message) => {
                        if job.saving {
                            self.saved = job.config;
                            self.saved_password = job.password;
                        }
                        self.error = None;
                        self.status.message = message;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            self.dirty = None;
        }
        if self.config != self.saved || self.password != self.saved_password {
            if self.dirty.get_or_insert_with(Instant::now).elapsed() > Duration::from_millis(700)
                && self.job.is_none()
            {
                if self.config.enabled && self.config.validate().is_err() {
                    self.error = self.config.validate().err();
                    self.dirty = Some(Instant::now());
                } else {
                    self.start(ctx, 0, None);
                }
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let mut changed = false;
        if self.poll.elapsed() > Duration::from_secs(1) {
            if let Ok(status) = config::root().and_then(|root| runtime::status(&root, &self.config))
            {
                changed =
                    status.last_success != self.status.last_success && status.last_success != 0;
                if self.config.enabled {
                    self.status = status;
                }
            }
            self.poll = Instant::now();
        }
        if self.job.is_some() || self.config.enabled {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        changed
    }
    pub fn flush_for_close(&mut self) -> Result<(), String> {
        if let Some(job) = self.job.as_ref().filter(|job| job.saving) {
            let result = job
                .receiver
                .recv_timeout(Duration::from_secs(2))
                .map_err(|_| "云同步设置仍在保存，请稍后关闭".to_string())?;
            if let Some(job) = self.job.take() {
                result?;
                self.saved = job.config;
                self.saved_password = job.password;
            }
        }
        if self.config != self.saved || self.password != self.saved_password {
            // Local credentials/configuration only; network sync runs in its helper.
            runtime::save(&self.config, &self.password)?;
            self.saved = self.config.clone();
            self.saved_password = self.password.clone();
            self.dirty = None;
        }
        Ok(())
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        super::app::card(ui, |ui| {
            super::app::switch_row(ui, &mut self.config.enabled, "云同步");
            ui.add_space(12.0);
            super::ai::configure_form(ui);
            let width = (ui.available_width() - 140.0).clamp(120.0, 360.0);
            egui::Grid::new("cloud-connection")
                .num_columns(2)
                .min_row_height(36.0)
                .spacing([20.0, 12.0])
                .show(ui, |ui| {
                    super::ai::form_label(ui, "提供商", 100.0);
                    ui.horizontal(|ui| {
                        let old = self.config.provider.clone();
                        egui::ComboBox::from_id_salt("cloud-provider")
                            .width((width - 36.0).max(120.0))
                            .selected_text(&self.config.provider)
                            .show_ui(ui, |ui| {
                                for (name, _, _) in PROVIDERS {
                                    ui.selectable_value(
                                        &mut self.config.provider,
                                        (*name).into(),
                                        *name,
                                    );
                                }
                            });
                        if old != self.config.provider {
                            if let Some((_, url, _)) =
                                PROVIDERS.iter().find(|p| p.0 == self.config.provider)
                            {
                                self.config.url = (*url).into();
                                self.password = runtime::password(&self.config).unwrap_or_default();
                            }
                        }
                        let help = PROVIDERS
                            .iter()
                            .find(|p| p.0 == self.config.provider)
                            .map_or(PROVIDERS[0].2, |p| p.2);
                        if ui
                            .small_button("↗")
                            .on_hover_text("打开云盘配置说明")
                            .clicked()
                        {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(help));
                        }
                    });
                    ui.end_row();
                    for (label, password, value) in [
                        ("WebDAV 地址", false, &mut self.config.url),
                        ("用户名", false, &mut self.config.username),
                        ("应用密码", true, &mut self.password),
                        ("设备名称", false, &mut self.config.device_name),
                    ] {
                        super::ai::form_label(ui, label, 100.0);
                        super::ai::provider_text(ui, value, width, password);
                        ui.end_row();
                    }
                });
            ui.add_space(12.0);
            if ui
                .add_enabled(self.job.is_none(), egui::Button::new("测试连接"))
                .clicked()
            {
                self.start(ui.ctx(), 1, None);
            }
        });
        ui.add_space(12.0);
        super::app::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("同步状态").strong());
                if self.status.busy || self.job.is_some() {
                    ui.spinner();
                }
            });
            if !self.config.enabled {
                ui.label("未启用");
            } else if !self.status.message.is_empty() {
                ui.label(&self.status.message);
            } else {
                ui.label("等待首次同步");
            }
            if self.status.last_success > 0 {
                use chrono::TimeZone;
                if let Some(time) = chrono::Local
                    .timestamp_opt(self.status.last_success as i64, 0)
                    .single()
                {
                    ui.label(
                        RichText::new(format!("最近同步：{}", time.format("%Y-%m-%d %H:%M")))
                            .size(13.0),
                    );
                }
            }
            if self.status.awaiting_merge {
                ui.label(format!(
                    "云端有 {} 台设备、{} 条学习记录、{} 条统计记录",
                    self.status.devices, self.status.words, self.status.statistics
                ));
                ui.label("首次合并采用云端设置，保留并合并本地词库和统计。");
            }
            let label = if self.status.awaiting_merge {
                "合并并同步"
            } else {
                "立即同步"
            };
            if ui
                .add_enabled(
                    self.config.enabled && self.job.is_none() && !self.status.busy,
                    egui::Button::new(label),
                )
                .clicked()
            {
                self.start(
                    ui.ctx(),
                    0,
                    Some(if self.status.awaiting_merge {
                        Command::Confirm
                    } else {
                        Command::Sync
                    }),
                );
            }
        });
        for conflict in self.status.conflicts.clone() {
            ui.add_space(12.0);
            super::app::card(ui, |ui| {
                ui.label(
                    RichText::new(format!("设置冲突 · {}", field_label(&conflict.key))).strong(),
                );
                ui.label(format!("本机与“{}”修改了同一项设置", conflict.device_name));
                ui.horizontal(|ui| {
                    for (label, remote, value) in [
                        ("保留本机", false, &conflict.local.value),
                        ("采用云端", true, &conflict.remote.value),
                    ] {
                        if ui
                            .add_enabled(self.job.is_none(), egui::Button::new(label))
                            .on_hover_text(value.to_string())
                            .clicked()
                        {
                            self.start(
                                ui.ctx(),
                                0,
                                Some(Command::Resolve {
                                    key: conflict.key.clone(),
                                    remote,
                                    stamp: conflict.remote.stamp.clone(),
                                }),
                            );
                        }
                    }
                });
            });
        }
        if let Some(error) = &self.error {
            ui.add_space(8.0);
            ui.colored_label(egui::Color32::DARK_RED, error);
        }
    }
}
impl Drop for CloudPage {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            if job.saving
                && job
                    .receiver
                    .recv_timeout(Duration::from_secs(2))
                    .is_ok_and(|r| r.is_ok())
            {
                self.saved = job.config;
                self.saved_password = job.password;
            }
        }
        if self.config != self.saved || self.password != self.saved_password {
            let _ = runtime::save(&self.config, &self.password);
        }
    }
}
fn field_label(key: &str) -> &str {
    match key {
        "input.scheme" => "拼音方案",
        "dictionary.enabled" => "扩展词库",
        "updates.auto_check" => "自动检查更新",
        "shortcuts" => "快捷键",
        "translation.model" => "翻译模型",
        "translation.target" => "翻译语言",
        "translation.reasoning" => "思考等级",
        _ if key.starts_with("providers/") => "AI 提供商",
        _ => "翻译设置",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cloud_form_fits_and_rendering_never_starts_network_work() {
        for width in [420.0, 620.0] {
            let config = Config {
                username: "fixture@example.invalid".into(),
                device_name: "测试设备".into(),
                ..Default::default()
            };
            let mut page = CloudPage {
                saved: config.clone(),
                config,
                password: String::new(),
                saved_password: String::new(),
                status: Status::default(),
                error: None,
                job: None,
                dirty: None,
                poll: Instant::now(),
                resume_pending: false,
            };
            let ctx = egui::Context::default();
            assert!(crate::app::chinese_fonts(&ctx).is_ok());
            ctx.set_theme(egui::Theme::Light);
            ctx.style_mut_of(egui::Theme::Light, |s| {
                s.text_styles
                    .insert(egui::TextStyle::Body, egui::FontId::proportional(16.0));
                s.text_styles
                    .insert(egui::TextStyle::Button, egui::FontId::proportional(16.0));
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
                        page.ui(ui);
                        right = ui.min_rect().right();
                    },
                );
                output.textures_delta.clear();
            }
            assert!(
                right <= width + 1.0,
                "Cloud form overflow: {right} > {width}"
            );
            assert!(page.job.is_none());
            assert_eq!(page.saved, page.config);
        }
    }
}
