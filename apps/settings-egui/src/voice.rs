//! Microphone test: owns its session, never inserts into an editor.
use eframe::egui;
use retype_ai::voice::{Command, Phase, Settings, Snapshot};
use std::{sync::mpsc, time::Duration};
pub struct Test {
    commands: mpsc::Sender<bool>,
    updates: mpsc::Receiver<Result<Snapshot, String>>,
    snapshot: Snapshot,
    error: Option<String>,
}
impl Test {
    pub fn start(settings: Settings) -> Self {
        let (tx, commands) = mpsc::channel();
        let (updates, rx) = mpsc::channel();
        let id = format!(
            "test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let spawn = std::thread::Builder::new()
            .name("retype-voice-test".into())
            .spawn(move || {
                let result = retype_ai::client::voice(Command::Start {
                    id: id.clone(),
                    settings,
                });
                let failed = result.is_err();
                let _ = updates.send(result);
                if failed {
                    return;
                }
                loop {
                    match commands.recv_timeout(Duration::from_millis(150)) {
                        Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            let _ = retype_ai::client::voice(Command::Cancel { id });
                            break;
                        }
                        Ok(true) => {
                            let _ = retype_ai::client::voice(Command::Stop { id: id.clone() });
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    let state = retype_ai::client::voice(Command::Poll { id: id.clone() });
                    let done = state.as_ref().map_or(true, |s| {
                        !matches!(s.phase, Phase::Recording | Phase::Recognizing)
                    });
                    if updates.send(state).is_err() || done {
                        break;
                    }
                }
            });
        Self {
            commands: tx,
            updates: rx,
            snapshot: Snapshot::default(),
            error: spawn.err().map(|_| "无法创建录音测试任务".into()),
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        while let Ok(result) = self.updates.try_recv() {
            match result {
                Ok(state) => self.snapshot = state,
                Err(e) => self.error = Some(e),
            }
        }
        ui.label(
            self.error
                .as_deref()
                .or(self.snapshot.error.as_deref())
                .unwrap_or(match self.snapshot.phase {
                    Phase::Recording => "录音中…",
                    Phase::Recognizing => "识别中…",
                    Phase::Done => "识别完成",
                    Phase::Error => "识别失败",
                    Phase::Cancelled => "已取消",
                }),
        );
        if self.snapshot.phase == Phase::Recording && self.error.is_none() {
            ui.add(egui::ProgressBar::new(self.snapshot.level).desired_width(180.0));
            if ui.button("结束录音").clicked() {
                let _ = self.commands.send(true);
            }
        }
        if matches!(self.snapshot.phase, Phase::Recording | Phase::Recognizing)
            && ui.button("取消").clicked()
        {
            let _ = self.commands.send(false);
        }
        if !self.snapshot.text.is_empty() {
            ui.label(&self.snapshot.text);
        }
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        let _ = self.commands.send(false);
    }
}
