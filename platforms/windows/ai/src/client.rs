use crate::protocol::{Operation, Request, Response};
use retype_learning::transport;
use std::os::windows::process::CommandExt;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
pub fn endpoint() -> Result<String, String> {
    Ok(format!(
        r"\\.\pipe\retype-ai-{}-v1",
        transport::user_sid().map_err(|e| e.to_string())?
    ))
}
pub fn exchange(name: &str, request: &Request) -> Result<Response, String> {
    let bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    let response = transport::exchange(name, &bytes).map_err(|e| e.to_string())?;
    serde_json::from_slice(&response).map_err(|e| e.to_string())
}
pub fn launch() -> Result<(), String> {
    let path = transport::read_machine_registry("ActiveDir")
        .map(PathBuf::from)
        .map(|p| p.join("retype-ai-host.exe"))
        .filter(|p| p.is_file())
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.join("retype-ai-host.exe")))
        })
        .ok_or("找不到翻译服务，请重新安装完整安装包")?;
    if !transport::is_app_container().unwrap_or(true)
        && std::process::Command::new(path)
            .creation_flags(0x08000000)
            .spawn()
            .is_ok()
    {
        return Ok(());
    }
    // AppContainer TIPs cannot directly create a full-trust child. The installed
    // per-user learning broker can launch only the AI binary beside itself.
    let sid = transport::user_sid().map_err(|_| "无法识别当前用户")?;
    let response = transport::exchange(&transport::pipe_name(&sid), br#"{"start_ai":true}"#)
        .map_err(|_| "无法启动翻译服务，请先打开设置或重新登录后重试")?;
    let response: serde_json::Value =
        serde_json::from_slice(&response).map_err(|_| "翻译后台启动响应无效")?;
    if response["ai_started"].as_bool() == Some(true) {
        Ok(())
    } else {
        Err("无法启动翻译服务，请确认安装包包含 retype-ai-host.exe".into())
    }
}
/// Runs on a voice/UI worker. Never send PCM across the learning-control pipe.
pub fn voice(command: crate::voice::Command) -> Result<crate::voice::Snapshot, String> {
    let name = endpoint()?;
    let request = Request::Voice(command);
    let mut result = exchange(&name, &request);
    if result.is_err() {
        launch()?;
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(100));
            result = exchange(&name, &request);
            if result.is_ok() {
                break;
            }
        }
    }
    match result? {
        Response::Voice(snapshot) => Ok(snapshot),
        Response::Error(e) => Err(e),
        _ => Err("语音服务响应无效".into()),
    }
}
/// Worker only. Each IPC roundtrip is bounded even for slow models.
pub fn run(operation: Operation, cancelled: &AtomicBool) -> Result<Response, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("已取消".into());
    }
    let name = endpoint()?;
    let id = crate::config::new_id();
    let start = Instant::now();
    let request = Request::Start {
        id: id.clone(),
        operation,
    };
    let mut response = exchange(&name, &request);
    if response.is_err() {
        launch()?;
        for _ in 0..40 {
            if cancelled.load(Ordering::Relaxed) {
                return Err("已取消".into());
            }
            std::thread::sleep(Duration::from_millis(100));
            response = exchange(&name, &request);
            if response.is_ok() {
                break;
            }
        }
    }
    let mut response = response.map_err(|_| "翻译服务未就绪，请稍后重试".to_string())?;
    loop {
        if cancelled.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(190) {
            let _ = exchange(&name, &Request::Cancel { id: id.clone() });
            return Err(if cancelled.load(Ordering::Relaxed) {
                "已取消"
            } else {
                "翻译超时"
            }
            .into());
        }
        match response {
            Response::Pending => {
                std::thread::sleep(Duration::from_millis(150));
                response = exchange(&name, &Request::Poll { id: id.clone() })?;
            }
            Response::Error(e) => return Err(e),
            other => return Ok(other),
        }
    }
}
