#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
fn serve() -> Result<(), Box<dyn std::error::Error>> {
    use retype_ai::{
        protocol::{Request, Response},
        service,
    };
    use retype_learning::transport;
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
        time::{Duration, Instant},
    };
    let sid = transport::user_sid()?;
    let name = retype_ai::client::endpoint()?;
    let listener = transport::Listener::new(&name, &sid)?;
    let jobs: Arc<Mutex<HashMap<String, (Instant, Response)>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let mut last = Instant::now();
    let active = Arc::new(AtomicUsize::new(0));
    let mut voice = retype_ai::voice_service::Controller::default();
    let mut microphone: Option<(String, Arc<std::sync::atomic::AtomicBool>)> = None;
    let directory = std::env::current_exe()?
        .parent()
        .ok_or("executable directory")?
        .to_path_buf();
    loop {
        if let Ok(mut map) = jobs.lock() {
            map.retain(|_, (time, _)| time.elapsed() < Duration::from_secs(240));
        }
        if active.load(Ordering::SeqCst) == 0 && jobs.lock().map(|j| j.is_empty()).unwrap_or(false)
        {
            if let Some(next) =
                transport::read_machine_registry("ActiveDir").map(std::path::PathBuf::from)
            {
                if next != directory && next.join("retype-ai-host.exe").is_file() {
                    break;
                }
            }
        }
        if last.elapsed() > Duration::from_secs(600)
            && jobs.lock().map(|j| j.is_empty()).unwrap_or(false)
        {
            break;
        }
        if let Ok(bytes) = listener.receive(&sid) {
            last = Instant::now();
            let result = (|| -> Result<Response, String> {
                let request: Request =
                    serde_json::from_slice(&bytes).map_err(|_| "请求格式错误")?;
                if let Request::Voice(command) = request {
                    use retype_ai::voice::Command;
                    let id = match &command {
                        Command::Start { id, .. }
                        | Command::Poll { id }
                        | Command::Stop { id }
                        | Command::Cancel { id } => id.clone(),
                    };
                    match command {
                        Command::Start { id, mut settings } => {
                            let config = retype_ai::config::Config::load()
                                .map_err(|_| "无法读取语音设置")?;
                            if settings.provider.is_empty() && settings.model.is_empty() {
                                settings = config.voice.clone();
                            }
                            let provider = settings.selected(&config)?.clone();
                            let key = retype_ai::secrets::key(&provider.id)
                                .map_err(|_| "无法读取语音 API Key")?;
                            let device = settings.microphone;
                            let session = voice.start(id.clone(), provider, settings, key)?;
                            if !microphone.as_ref().is_some_and(|(old, _)| old == &id) {
                                let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
                                if let Err(e) = retype_ai::microphone::start(
                                    Arc::clone(&session),
                                    device,
                                    Arc::clone(&stop),
                                ) {
                                    session.fail(&e);
                                    return Err(e);
                                }
                                microphone = Some((id, stop));
                            }
                        }
                        Command::Stop { id } => {
                            voice.get(&id)?;
                            if let Some((old, stop)) = &microphone {
                                if old == &id {
                                    stop.store(true, Ordering::Release);
                                }
                            }
                        }
                        Command::Cancel { id } => {
                            voice.get(&id)?.cancel();
                        }
                        Command::Poll { .. } => {}
                    }
                    return Ok(Response::Voice(voice.get(&id)?.snapshot()));
                }
                let mut map = jobs.lock().map_err(|_| "服务状态异常")?;
                map.retain(|_, (time, _)| time.elapsed() < Duration::from_secs(240));
                match request {
                    Request::Voice(_) => unreachable!(),
                    Request::Start { id, operation } => {
                        if id.len() > 128 {
                            return Err("请求 ID 过长".into());
                        }
                        if let Some((_, response)) = map.get(&id) {
                            return Ok(response.clone());
                        }
                        if map.len() >= 16 || active.load(Ordering::SeqCst) >= 4 {
                            return Err("请求过多，请稍后重试".into());
                        }
                        map.insert(id.clone(), (Instant::now(), Response::Pending));
                        let jobs = Arc::clone(&jobs);
                        active.fetch_add(1, Ordering::SeqCst);
                        let task_active = Arc::clone(&active);
                        if std::thread::Builder::new()
                            .name("retype-translation".into())
                            .spawn(move || {
                                let response =
                                    std::panic::catch_unwind(|| service::perform(operation))
                                        .unwrap_or_else(|_| Err("翻译服务异常".into()))
                                        .unwrap_or_else(Response::Error);
                                task_active.fetch_sub(1, Ordering::SeqCst);
                                if let Ok(mut map) = jobs.lock() {
                                    if let Some(entry) = map.get_mut(&id) {
                                        entry.1 = response;
                                    }
                                }
                            })
                            .is_err()
                        {
                            active.fetch_sub(1, Ordering::SeqCst);
                            return Err("无法创建翻译任务".into());
                        }
                        Ok(Response::Pending)
                    }
                    Request::Poll { id } => {
                        let response = map
                            .get(&id)
                            .map(|(_, r)| r.clone())
                            .ok_or("翻译任务已失效")?;
                        if !matches!(response, Response::Pending) {
                            map.remove(&id);
                        }
                        Ok(response)
                    }
                    Request::Cancel { id } => {
                        map.remove(&id);
                        Ok(Response::Error("已取消".into()))
                    }
                }
            })()
            .unwrap_or_else(Response::Error);
            let _ = listener.respond(&serde_json::to_vec(&result)?);
        }
        listener.disconnect();
    }
    Ok(())
}
fn main() {
    #[cfg(windows)]
    if serve().is_err() {
        std::process::exit(1);
    }
}
