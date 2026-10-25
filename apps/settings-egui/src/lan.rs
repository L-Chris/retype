use eframe::egui;
use retype_sync::lan::{self, Command, Config, Status};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

pub struct LanPage {
    config: Config,
    status: Status,
    last_poll: Instant,
    error: Option<String>,
    join_code: String,
    join_address: String,
    editing_peer: Option<(String, String)>,
    listen_port: String,
    show_join: bool,
    job: Option<mpsc::Receiver<Result<(), String>>>,
}
impl LanPage {
    pub fn new() -> Self {
        Self {
            config: Config::default(),
            status: Status::default(),
            last_poll: Instant::now() - Duration::from_secs(2),
            error: None,
            join_code: String::new(),
            join_address: String::new(),
            editing_peer: None,
            listen_port: String::new(),
            show_join: false,
            job: None,
        }
    }
    fn command(&mut self, ctx: &egui::Context, command: Command) {
        if self.job.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.job = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(lan::command(command));
            ctx.request_repaint();
        });
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if let Some(result) = self.job.as_ref().and_then(|job| job.try_recv().ok()) {
            self.error = result.err();
            self.job = None;
            self.last_poll = Instant::now() - Duration::from_secs(2);
        }
        if self.last_poll.elapsed() >= Duration::from_secs(1) {
            match lan::root()
                .and_then(|root| lan::load_at(&root).map(|config| (config, lan::status_at(&root))))
            {
                Ok((config, status)) => {
                    if self.listen_port.is_empty() {
                        self.listen_port = if config.port == 0 {
                            lan::DEFAULT_PORT
                        } else {
                            config.port
                        }
                        .to_string();
                    }
                    self.config = config;
                    self.status = status;
                }
                Err(error) => self.error = Some(error),
            }
            self.last_poll = Instant::now();
        }
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        egui::Frame::new()
            .fill(egui::Color32::WHITE)
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgb(227, 233, 236),
            ))
            .corner_radius(16)
            .inner_margin(20)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.add_enabled_ui(self.job.is_none(), |ui| {
                    let mut enabled = self.config.enabled;
                    ui.horizontal(|ui| {
                        ui.label("文字剪贴板同步");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::app::toggle(ui, &mut enabled, "文字剪贴板同步") {
                                self.command(ui.ctx(), Command::Enable(enabled));
                            }
                        });
                    });
                    ui.add_space(12.0);
                    ui.label(if self.status.message.is_empty() {
                        if enabled {
                            "启动中…"
                        } else {
                            "已关闭"
                        }
                    } else {
                        &self.status.message
                    });
                    if self.config.enabled {
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            if ui.button("查看匹配码").clicked() {
                                self.show_join = false;
                                self.command(ui.ctx(), Command::Pair);
                            }
                            if ui.button("关联设备").clicked() {
                                self.show_join = true;
                                self.command(ui.ctx(), Command::Cancel);
                            }
                        });
                        if self.status.mode == "show" && !self.status.code.is_empty() {
                            ui.add_space(12.0);
                            ui.heading(&self.status.code);
                            let seconds = self
                                .status
                                .expires
                                .saturating_sub(retype_sync::runtime::now() * 1000)
                                / 1000;
                            ui.label(format!("请在另一台设备输入此码 · {seconds} 秒后失效"));
                            if ui.button("取消").clicked() {
                                self.command(ui.ctx(), Command::Cancel);
                            }
                        }
                        if self.show_join {
                            ui.add_space(12.0);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.join_address)
                                    .hint_text("设备地址（可选），例如 home:50206；留空自动发现")
                                    .desired_width(f32::INFINITY),
                            );
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.join_code)
                                        .hint_text("输入另一台设备的 6 位匹配码")
                                        .desired_width(270.0)
                                        .char_limit(6),
                                );
                                self.join_code.retain(|c| c.is_ascii_digit());
                                if ui
                                    .add_enabled(
                                        retype_sync::pairing::valid_code(&self.join_code)
                                            && self.status.mode != "join",
                                        egui::Button::new("关联"),
                                    )
                                    .clicked()
                                {
                                    let command = if self.join_address.trim().is_empty() {
                                        Command::Join(self.join_code.clone())
                                    } else {
                                        Command::JoinAddress {
                                            code: self.join_code.clone(),
                                            address: self.join_address.clone(),
                                        }
                                    };
                                    self.command(ui.ctx(), command);
                                    self.join_code.clear();
                                }
                                if ui.button("取消").clicked() {
                                    self.show_join = false;
                                    self.join_code.clear();
                                    self.command(ui.ctx(), Command::Cancel);
                                }
                            });
                        }
                        ui.add_space(20.0);
                        for peer in self.config.peers.clone() {
                            ui.horizontal(|ui| {
                                ui.label(&peer.name);
                                ui.label(if self.status.online.contains(&peer.id) {
                                    "已连接"
                                } else {
                                    "离线"
                                });
                                if ui.button("连接地址…").clicked() {
                                    self.editing_peer =
                                        Some((peer.id.clone(), peer.addresses.join(", ")));
                                }
                                if ui.button("解除关联").clicked() {
                                    self.command(ui.ctx(), Command::Remove(peer.id.clone()));
                                }
                            });
                            if let Some(address) = self.status.connections.get(&peer.id) {
                                ui.small(address);
                            }
                            if self
                                .editing_peer
                                .as_ref()
                                .is_some_and(|(id, _)| id == &peer.id)
                            {
                                let mut edit = self.editing_peer.take().unwrap_or_default();
                                ui.add(
                                    egui::TextEdit::singleline(&mut edit.1)
                                        .desired_width(f32::INFINITY)
                                        .hint_text("IP 或域名，可附带端口；多个地址用逗号分隔"),
                                );
                                ui.small("补充 Tailscale 地址即可继续使用原绑定，无需匹配码。");
                                let mut close = false;
                                ui.horizontal(|ui| {
                                    if ui.button("保存地址").clicked() {
                                        self.command(
                                            ui.ctx(),
                                            Command::Address {
                                                id: edit.0.clone(),
                                                address: edit.1.clone(),
                                            },
                                        );
                                        close = true;
                                    }
                                    if ui.button("取消").clicked() {
                                        close = true;
                                    }
                                });
                                if !close {
                                    self.editing_peer = Some(edit);
                                }
                            }
                            ui.add_space(8.0);
                        }
                    }
                    ui.add_space(12.0);
                    ui.label(
                        "支持物理局域网和 Tailscale；指定地址时，请允许路由及防火墙访问同步端口。",
                    );
                    ui.collapsing("本机连接地址与端口", |ui| {
                        for address in self.status.addresses.clone() {
                            ui.horizontal(|ui| {
                                ui.monospace(&address);
                                if ui.small_button("复制").clicked() {
                                    ui.ctx().copy_text(address);
                                }
                            });
                        }
                        ui.horizontal(|ui| {
                            ui.label("监听端口");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.listen_port)
                                    .desired_width(90.0)
                                    .char_limit(5),
                            );
                            if ui.button("应用").clicked() {
                                match self.listen_port.parse::<u16>() {
                                    Ok(port) if port > 0 => {
                                        self.command(ui.ctx(), Command::Port(port))
                                    }
                                    _ => self.error = Some("端口范围为 1–65535".into()),
                                }
                            }
                        });
                    });
                });
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::RED, error);
                }
            });
    }
}
