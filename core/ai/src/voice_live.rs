//! Gemini Live transcription. One bounded socket/session; no file-upload fallback.
use crate::{
    config::{ApiKind, Provider},
    voice::{self, Phase, Settings, Snapshot},
    voice_service::{update, Input},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    io,
    net::{TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex,
    },
    time::{Duration, Instant},
};
use tungstenite::{client::IntoClientRequest, stream::MaybeTlsStream, Message, WebSocket};

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

pub(crate) fn http_base(provider: &Provider) -> Result<String, String> {
    let value = provider.base_url.trim().trim_end_matches('/');
    let url = url::Url::parse(value).map_err(|_| "语音接口地址无效")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || (url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("语音接口需使用 HTTPS，且不能包含凭据或查询参数".into());
    }
    Ok(value.into())
}
pub(crate) fn model_id(model: &str) -> Result<&str, String> {
    let model = model.strip_prefix("models/").unwrap_or(model);
    if model.is_empty()
        || model.len() > 512
        || !model
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
    {
        return Err("Gemini 模型 ID 格式不正确".into());
    }
    Ok(model)
}
fn socket_error(error: tungstenite::Error) -> String {
    // Never format transport errors: request URLs may contain a Google API key.
    match error {
        tungstenite::Error::Http(response) => match response.status().as_u16() {
            401 | 403 => "实时接口鉴权失败，请检查 API Key 和模型权限".into(),
            404 => "提供商未提供 Gemini Live 接口，请检查网关是否支持".into(),
            code => format!("实时接口返回 HTTP {code}"),
        },
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            "实时连接提前关闭，识别未完成".into()
        }
        _ => "实时连接失败或超时，请检查网络与接口配置".into(),
    }
}
fn timeout(socket: &mut Socket, duration: Duration) -> Result<(), String> {
    let stream = match socket.get_mut() {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::Rustls(s) => &mut s.sock,
        _ => return Err("不支持的实时连接类型".into()),
    };
    stream
        .set_read_timeout(Some(duration))
        .map_err(|_| "无法配置实时连接".into())
}
fn send(socket: &mut Socket, value: Value) -> Result<(), String> {
    socket
        .send(Message::Text(value.to_string().into()))
        .map_err(socket_error)
}
fn receive(socket: &mut Socket) -> Result<Option<Value>, String> {
    match socket.read() {
        Ok(message) if message.is_text() || message.is_binary() => {
            // Google can send UTF-8 JSON in binary WebSocket frames, unlike the
            // text-only local fixtures. Both carry the same Live protocol events.
            let value: Value =
                serde_json::from_slice(&message.into_data()).map_err(|_| "实时响应格式错误")?;
            if value.get("error").is_some() {
                return Err("实时模型拒绝请求，请检查模型权限和接口配置".into());
            }
            Ok(Some(value))
        }
        Ok(Message::Close(_)) => Err("实时连接提前关闭，识别未完成".into()),
        Ok(_) => Ok(None),
        Err(tungstenite::Error::Io(e))
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(socket_error(e)),
    }
}
fn connect(
    provider: &Provider,
    settings: &Settings,
    key: &str,
    cancelled: &AtomicBool,
) -> Result<Socket, String> {
    let base = http_base(provider)?;
    let model = model_id(&settings.model)?;
    let mut url = url::Url::parse(&base).map_err(|_| "实时接口地址无效")?;
    let prefix = url.path().trim_end_matches('/').to_owned();
    let prefix = ["/v1beta", "/v1alpha", "/v1"]
        .iter()
        .find_map(|suffix| prefix.strip_suffix(suffix))
        .unwrap_or(&prefix);
    url.set_path(&format!(
        "{prefix}/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent"
    ));
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme).map_err(|_| "实时接口地址无效")?;
    if provider.kind == ApiKind::Gemini {
        url.query_pairs_mut().append_pair("key", key);
    }
    let host = url.host_str().ok_or("实时接口地址无效")?;
    let port = url.port_or_known_default().ok_or("实时接口端口无效")?;
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|_| "实时接口域名解析失败")?;
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut connected = None;
    for address in addresses {
        if cancelled.load(Ordering::Acquire) {
            return Err("已取消".into());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        if let Ok(stream) =
            TcpStream::connect_timeout(&address, remaining.min(Duration::from_secs(3)))
        {
            connected = Some(stream);
            break;
        }
    }
    let stream = connected.ok_or("无法连接实时接口")?;
    stream.set_nodelay(true).map_err(|_| "无法配置实时连接")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(8)))
        .map_err(|_| "无法配置实时连接")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| "无法配置实时连接")?;
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| "实时接口地址无效")?;
    if provider.kind != ApiKind::Gemini {
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {key}")
                .parse()
                .map_err(|_| "API Key 格式无效")?,
        );
    }
    let config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(1024 * 1024))
        .max_frame_size(Some(1024 * 1024));
    let (mut socket, _) = tungstenite::client_tls_with_config(request, stream, Some(config), None)
        .map_err(|error| match error {
            tungstenite::HandshakeError::Failure(error) => socket_error(error),
            _ => "实时握手超时，请检查网络与接口配置".into(),
        })?;
    timeout(&mut socket, Duration::from_secs(1))?;
    let languages: Vec<&str> = match settings.language.as_str() {
        "zh" => vec!["cmn-Hans-CN"],
        "en" => vec!["en-US"],
        _ => vec![],
    };
    send(
        &mut socket,
        json!({"setup":{
            "model":format!("models/{model}"),
            "generationConfig":{"responseModalities":["TEXT"]},
            "inputAudioTranscription":{"languageCodes":languages,"mode":if settings.tidy {"SMART"} else {"VERBATIM"}},
            "realtimeInputConfig":{"automaticActivityDetection":{"disabled":true}}
        }}),
    )?;
    let setup_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < setup_deadline {
        if cancelled.load(Ordering::Acquire) {
            let _ = socket.close(None);
            return Err("已取消".into());
        }
        if receive(&mut socket)?.is_some_and(|v| v.get("setupComplete").is_some()) {
            timeout(&mut socket, Duration::from_millis(50))?;
            return Ok(socket);
        }
    }
    Err("实时模型初始化超时".into())
}

/// Test authentication/model setup without opening a microphone or generating text.
pub(crate) fn test(provider: &Provider, model: &str, key: &str) -> Result<(), String> {
    let mut socket = connect(
        provider,
        &Settings {
            model: model.into(),
            ..Default::default()
        },
        key,
        &AtomicBool::new(false),
    )?;
    let _ = socket.close(None);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    provider: &Provider,
    settings: &Settings,
    key: &str,
    receiver: mpsc::Receiver<Input>,
    snapshot: &Mutex<Snapshot>,
    cancelled: &AtomicBool,
    stopping: &AtomicBool,
) -> Result<(), String> {
    let mut socket = connect(provider, settings, key, cancelled)?;
    send(&mut socket, json!({"realtimeInput":{"activityStart":{}}}))?;
    let mut samples = 0usize;
    let mut has_speech = false;
    let mut ended = None::<Instant>;
    let mut text = Transcript::default();
    loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = socket.close(None);
            return Ok(());
        }
        if ended.is_none() {
            for _ in 0..8 {
                match receiver.try_recv() {
                    Ok(Input::Frame(frame)) => {
                        let frame = &frame[..frame.len().min(voice::MAX_SAMPLES - samples)];
                        samples += frame.len();
                        let rms = (frame.iter().map(|v| (*v as f64).powi(2)).sum::<f64>()
                            / frame.len().max(1) as f64)
                            .sqrt();
                        has_speech |= rms > 140.0;
                        update(snapshot, cancelled, |s| {
                            s.level = (rms / 3000.0).min(1.0) as f32;
                            s.seconds = (samples / voice::RATE) as u32;
                        });
                        let pcm: Vec<u8> = frame.iter().flat_map(|v| v.to_le_bytes()).collect();
                        send(
                            &mut socket,
                            json!({"realtimeInput":{"audio":{"mimeType":"audio/pcm;rate=16000","data":STANDARD.encode(pcm)}}}),
                        )?;
                        if samples < voice::MAX_SAMPLES {
                            continue;
                        }
                    }
                    Ok(Input::Stop) => {}
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(_) => return Err("录音会话已断开".into()),
                }
                stopping.store(true, Ordering::Release);
                if !has_speech {
                    let _ = socket.close(None);
                    return Err("没有检测到语音".into());
                }
                update(snapshot, cancelled, |s| {
                    s.phase = Phase::Recognizing;
                    s.level = 0.0;
                });
                send(&mut socket, json!({"realtimeInput":{"activityEnd":{}}}))?;
                ended = Some(Instant::now());
                // Completion must belong to the final explicitly-ended activity.
                text.turn_complete = false;
                text.final_after_end = false;
                break;
            }
        }
        if let Some(value) = receive(&mut socket)? {
            text.accept(&value, ended.is_some())?;
            update(snapshot, cancelled, |s| s.text = text.preview());
        }
        if ended.is_some() && text.turn_complete && text.final_after_end {
            let final_text = text.stable.trim();
            if final_text.is_empty() {
                return Err("没有识别到文字".into());
            }
            update(snapshot, cancelled, |s| {
                s.text = final_text.into();
                s.phase = Phase::Done;
            });
            let _ = socket.close(None);
            return Ok(());
        }
        if ended.is_some_and(|t| t.elapsed() > Duration::from_secs(20)) {
            return Err("等待最终转写超时，预览文字未提交".into());
        }
    }
}

#[derive(Default)]
struct Transcript {
    stable: String,
    interim: String,
    turn_complete: bool,
    final_after_end: bool,
}
impl Transcript {
    fn accept(&mut self, value: &Value, ended: bool) -> Result<(), String> {
        let content = &value["serverContent"];
        if content["interrupted"] == true {
            return Err("实时转写被中断，预览文字未提交".into());
        }
        if let Some(text) = content["interimInputTranscription"]["text"].as_str() {
            self.interim = text.into();
        }
        if let Some(text) = content["inputTranscription"]["text"].as_str() {
            self.stable.push_str(text);
            self.interim.clear();
            self.final_after_end |= ended;
        }
        // Dedicated Transcribe Live emits generationComplete without turnComplete
        // (verified against the official endpoint); conversational Live can use either.
        self.turn_complete |=
            ended && (content["turnComplete"] == true || content["generationComplete"] == true);
        if self.stable.len() + self.interim.len() > 128 * 1024 {
            return Err("语音结果过长".into());
        }
        Ok(())
    }
    fn preview(&self) -> String {
        format!("{}{}", self.stable, self.interim)
    }
}

#[cfg(test)]
#[path = "voice_live_tests.rs"]
mod tests;
