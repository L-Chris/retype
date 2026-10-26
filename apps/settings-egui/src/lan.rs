use crate::app::{
    card, icon_button, primary_button, toggle, Icon, ACCENT, ACCENT_SOFT, MUTED, ONLINE,
};
use eframe::egui::{self, Color32, RichText, Vec2};
use retype_sync::lan::{self, Command, Config, Status};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

fn format_code(code: &str) -> String {
    if code.len() != 6 {
        return code.to_owned();
    }
    let mut parts = Vec::with_capacity(7);
    for (i, ch) in code.chars().enumerate() {
        parts.push(ch.to_string());
        if i == 2 {
            parts.push(" ".into());
        }
    }
    parts.join(" ")
}

fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

pub struct LanPage {
    config: Config,
    status: Status,
    last_poll: Instant,
    error: Option<String>,
    join_code: String,
    join_address: String,
    editing_peer: Option<(String, String)>,
    editing_port: bool,
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
            editing_port: false,
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
                    if !self.editing_port {
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

        let busy = self.job.is_some();
        let enabled = self.config.enabled;

        // Card 1: Master Switch & Pairing
        card(ui, |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                let mut next_enabled = enabled;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("剪贴板同步").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if toggle(ui, &mut next_enabled, "剪贴板同步") {
                            self.command(ui.ctx(), Command::Enable(next_enabled));
                        }
                    });
                });

                if enabled {
                    ui.separator();
                    let showing_code = self.status.mode == "show" && !self.status.code.is_empty();
                    ui.horizontal(|ui| {
                        status_dot(
                            ui,
                            if !self.status.online.is_empty() {
                                ONLINE
                            } else {
                                ACCENT
                            },
                        );
                        let summary = if !self.status.message.is_empty() {
                            self.status.message.clone()
                        } else if !self.status.online.is_empty() {
                            format!("已连接 {} 台设备", self.status.online.len())
                        } else {
                            "等待设备连接".into()
                        };
                        ui.label(RichText::new(summary).size(14.0).color(MUTED));

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let join_clicked = if self.show_join {
                                primary_button(ui, "关联", true)
                            } else {
                                ui.button("关联")
                                    .on_hover_text("输入匹配码关联其他设备")
                                    .clicked()
                            };
                            if join_clicked {
                                self.show_join = !self.show_join;
                                if self.show_join {
                                    self.command(ui.ctx(), Command::Cancel);
                                }
                            }
                            let pair_clicked = if showing_code {
                                primary_button(ui, "匹配码", true)
                            } else {
                                ui.button("匹配码")
                                    .on_hover_text("生成 6 位配对匹配码")
                                    .clicked()
                            };
                            if pair_clicked {
                                self.show_join = false;
                                self.command(ui.ctx(), Command::Pair);
                            }
                        });
                    });

                    if showing_code {
                        ui.add_space(6.0);
                        egui::Frame::new()
                            .fill(ACCENT_SOFT)
                            .corner_radius(10)
                            .inner_margin(14)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format_code(&self.status.code))
                                            .monospace()
                                            .size(22.0)
                                            .strong()
                                            .color(ACCENT),
                                    );
                                    let seconds = self
                                        .status
                                        .expires
                                        .saturating_sub(retype_sync::runtime::now() * 1000)
                                        / 1000;
                                    ui.label(
                                        RichText::new(format!("{seconds}s 后失效"))
                                            .size(12.0)
                                            .color(MUTED),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if icon_button(
                                                ui,
                                                Icon::Close,
                                                "关闭匹配码",
                                                false,
                                                false,
                                                true,
                                            ) {
                                                self.command(ui.ctx(), Command::Cancel);
                                            }
                                        },
                                    );
                                });
                            });
                    }

                    if self.show_join {
                        ui.separator();
                        crate::ai::configure_form(ui);
                        let code_width = (ui.available_width() - 188.0).clamp(110.0, 260.0);
                        let addr_width = (ui.available_width() - 104.0).clamp(140.0, 344.0);
                        egui::Grid::new("lan-join-grid")
                            .num_columns(2)
                            .min_row_height(36.0)
                            .spacing([16.0, 10.0])
                            .show(ui, |ui| {
                                crate::ai::form_label(ui, "匹配码", 80.0);
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.join_code)
                                            .hint_text("输入 6 位数字")
                                            .desired_width(code_width)
                                            .min_size(Vec2::new(code_width, 36.0))
                                            .vertical_align(egui::Align::Center)
                                            .char_limit(6),
                                    );
                                    self.join_code.retain(|c| c.is_ascii_digit());
                                    let can_submit =
                                        retype_sync::pairing::valid_code(&self.join_code)
                                            && self.status.mode != "join";
                                    if icon_button(
                                        ui,
                                        Icon::Check,
                                        "确认关联",
                                        true,
                                        false,
                                        can_submit,
                                    ) {
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
                                        self.show_join = false;
                                    }
                                    if icon_button(ui, Icon::Close, "取消", false, false, true) {
                                        self.show_join = false;
                                        self.join_code.clear();
                                        self.command(ui.ctx(), Command::Cancel);
                                    }
                                });
                                ui.end_row();

                                crate::ai::form_label(ui, "指定地址", 80.0);
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.join_address)
                                        .hint_text("选填，如 100.64.0.1:50206（局域网留空）")
                                        .desired_width(addr_width)
                                        .min_size(Vec2::new(addr_width, 36.0))
                                        .vertical_align(egui::Align::Center),
                                );
                                ui.end_row();
                            });
                    }
                }
            });
        });

        // Card 2: Associated Peers
        if enabled || !self.config.peers.is_empty() {
            ui.add_space(12.0);
            card(ui, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    ui.label(RichText::new("已关联设备").size(13.0).strong().color(MUTED));
                    if self.config.peers.is_empty() {
                        ui.label(RichText::new("暂无关联设备").size(14.0).color(MUTED));
                    } else {
                        for peer in self.config.peers.clone() {
                            ui.separator();
                            let online = self.status.online.contains(&peer.id);
                            let addr_text = self
                                .status
                                .connections
                                .get(&peer.id)
                                .cloned()
                                .unwrap_or_else(|| {
                                    if peer.addresses.is_empty() {
                                        "自动发现".into()
                                    } else {
                                        peer.addresses.join(", ")
                                    }
                                });
                            let is_editing = self
                                .editing_peer
                                .as_ref()
                                .is_some_and(|(id, _)| id == &peer.id);

                            ui.horizontal(|ui| {
                                let info_width = (ui.available_width() - 96.0).max(120.0);
                                ui.allocate_ui_with_layout(
                                    Vec2::new(info_width, 38.0),
                                    egui::Layout::top_down(egui::Align::LEFT),
                                    |ui| {
                                        ui.horizontal(|ui| {
                                            status_dot(
                                                ui,
                                                if online {
                                                    ONLINE
                                                } else {
                                                    Color32::from_rgb(190, 200, 206)
                                                },
                                            );
                                            ui.label(RichText::new(&peer.name).strong());
                                            ui.label(
                                                RichText::new(if online {
                                                    "已连接"
                                                } else {
                                                    "离线"
                                                })
                                                .size(12.0)
                                                .color(if online { ACCENT } else { MUTED }),
                                            );
                                        });
                                        ui.label(
                                            RichText::new(addr_text)
                                                .monospace()
                                                .size(12.0)
                                                .color(MUTED),
                                        );
                                    },
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if icon_button(
                                            ui,
                                            Icon::Trash,
                                            "解除关联",
                                            false,
                                            true,
                                            true,
                                        ) {
                                            self.command(
                                                ui.ctx(),
                                                Command::Remove(peer.id.clone()),
                                            );
                                        }
                                        if icon_button(
                                            ui,
                                            Icon::Sliders,
                                            "连接地址",
                                            is_editing,
                                            false,
                                            true,
                                        ) {
                                            if is_editing {
                                                self.editing_peer = None;
                                            } else {
                                                self.editing_peer = Some((
                                                    peer.id.clone(),
                                                    peer.addresses.join(", "),
                                                ));
                                            }
                                        }
                                    },
                                );
                            });

                            if is_editing {
                                let mut edit = self.editing_peer.take().unwrap_or_default();
                                let edit_width = (ui.available_width() - 92.0).max(120.0);
                                let mut close = false;
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut edit.1)
                                            .desired_width(edit_width)
                                            .min_size(Vec2::new(edit_width, 36.0))
                                            .vertical_align(egui::Align::Center)
                                            .hint_text("IP 或域名:端口，多个用逗号分隔"),
                                    );
                                    if icon_button(ui, Icon::Check, "保存地址", true, false, true)
                                    {
                                        self.command(
                                            ui.ctx(),
                                            Command::Address {
                                                id: edit.0.clone(),
                                                address: edit.1.clone(),
                                            },
                                        );
                                        close = true;
                                    }
                                    if icon_button(ui, Icon::Close, "取消", false, false, true) {
                                        close = true;
                                    }
                                });
                                if !close {
                                    self.editing_peer = Some(edit);
                                }
                            }
                        }
                    }
                });
            });
        }

        // Card 3: Local Port & Addresses
        if enabled {
            ui.add_space(12.0);
            card(ui, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal(|ui| {
                        crate::ai::form_label(ui, "监听端口", 80.0);
                        if self.editing_port {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.listen_port)
                                    .desired_width(100.0)
                                    .min_size(Vec2::new(100.0, 36.0))
                                    .vertical_align(egui::Align::Center)
                                    .char_limit(5),
                            );
                            if icon_button(ui, Icon::Check, "应用端口", true, false, true) {
                                match self.listen_port.parse::<u16>() {
                                    Ok(port) if port > 0 => {
                                        self.editing_port = false;
                                        self.command(ui.ctx(), Command::Port(port));
                                    }
                                    _ => self.error = Some("端口范围为 1–65535".into()),
                                }
                            }
                            if icon_button(ui, Icon::Close, "取消", false, false, true) {
                                self.editing_port = false;
                                self.listen_port = if self.config.port == 0 {
                                    lan::DEFAULT_PORT
                                } else {
                                    self.config.port
                                }
                                .to_string();
                            }
                        } else {
                            ui.label(RichText::new(&self.listen_port).monospace().size(14.0));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if icon_button(
                                        ui,
                                        Icon::Edit,
                                        "修改监听端口",
                                        false,
                                        false,
                                        true,
                                    ) {
                                        self.editing_port = true;
                                    }
                                },
                            );
                        }
                    });

                    if !self.status.addresses.is_empty() {
                        ui.separator();
                        for (idx, address) in self.status.addresses.clone().into_iter().enumerate()
                        {
                            ui.horizontal(|ui| {
                                crate::ai::form_label(
                                    ui,
                                    if idx == 0 { "本机地址" } else { "" },
                                    80.0,
                                );
                                ui.label(
                                    RichText::new(&address).monospace().size(13.0).color(MUTED),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if icon_button(
                                            ui,
                                            Icon::Copy,
                                            "复制地址",
                                            false,
                                            false,
                                            true,
                                        ) {
                                            ui.ctx().copy_text(address);
                                        }
                                    },
                                );
                            });
                        }
                    }
                });
            });
        }

        if let Some(error) = &self.error {
            ui.add_space(8.0);
            ui.colored_label(Color32::DARK_RED, error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lan_cards_fit_normal_and_minimum_widths() {
        for width in [420.0, 620.0] {
            let mut page = LanPage {
                config: Config {
                    enabled: true,
                    port: 50206,
                    peers: vec![retype_sync::lan::Peer {
                        id: "peer-1".into(),
                        name: "MacBook Pro".into(),
                        addresses: vec!["100.64.0.1:50206".into()],
                    }],
                    ..Default::default()
                },
                status: Status {
                    mode: "show".into(),
                    code: "482910".into(),
                    expires: retype_sync::runtime::now() * 1000 + 120_000,
                    online: vec!["peer-1".into()],
                    addresses: vec!["192.168.1.20:50206".into()],
                    ..Default::default()
                },
                last_poll: Instant::now(),
                error: None,
                join_code: "123456".into(),
                join_address: String::new(),
                editing_peer: Some(("peer-1".into(), "100.64.0.1:50206".into())),
                editing_port: true,
                listen_port: "50206".into(),
                show_join: true,
                job: None,
            };
            let ctx = egui::Context::default();
            assert!(crate::app::chinese_fonts(&ctx).is_ok());
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
                "lan page extends to {right} at width {width}"
            );
            assert!(page.job.is_none());
        }
    }
}
