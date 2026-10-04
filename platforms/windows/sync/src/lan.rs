//! LAN-only clipboard host. Secrets never enter status files or cloud snapshots.
use crate::{
    clipboard_protocol::{Clip, Decoder, Journal},
    config, Result,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    ServerConfig, ServerConnection, StreamOwned,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{IpAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
#[cfg(test)]
#[path = "lan_tests.rs"]
mod tests;

#[derive(Clone, Serialize, Deserialize)]
pub struct Peer {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    pub port: u16,
    pub id: String,
    pub name: String,
    pub peers: Vec<Peer>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 0,
            id: uuid::Uuid::new_v4().simple().to_string(),
            name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows".into()),
            peers: Vec::new(),
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Status {
    pub message: String,
    pub port: u16,
    pub addresses: Vec<String>,
    pub online: Vec<String>,
    pub pending: Option<Pending>,
    pub pairing: bool,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub expires: u64,
    #[serde(default)]
    pub mode: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Pending {
    pub id: String,
    pub name: String,
    pub session: String,
}
#[derive(Serialize, Deserialize)]
pub enum Command {
    Enable(bool),
    Pair,
    Join(String),
    Cancel,
    Remove(String),
}
type Mailbox = Arc<Mutex<Option<Clip>>>;
struct State {
    config: Config,
    status: Status,
    pair_until: Option<Instant>,
    pending_until: Option<Instant>,
    journal: Journal,
    seq: u64,
    senders: Vec<(String, Mailbox)>,
    pair_code: String,
    attempts: u8,
}
struct SessionGuard<'a> {
    state: &'a Mutex<State>,
    peer: Option<String>,
    pending: Option<String>,
}
impl Drop for SessionGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(peer) = &self.peer {
                s.status.online.retain(|id| id != peer);
                s.senders.retain(|(id, _)| id != peer);
            }
            if self
                .pending
                .as_ref()
                .is_some_and(|id| s.status.pending.as_ref().is_some_and(|p| &p.session == id))
            {
                s.status.pending = None;
                s.pending_until = None;
            }
        }
    }
}
fn now() -> u64 {
    crate::runtime::now().saturating_mul(1000)
}
pub fn root() -> Result<PathBuf> {
    Ok(config::root()?.join("lan"))
}
pub fn load_at(root: &Path) -> Result<Config> {
    match std::fs::read(root.join("config.json")) {
        Ok(b) => serde_json::from_slice(&b).map_err(|_| "跨设备配置损坏".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(_) => Err("无法读取跨设备配置".into()),
    }
}
pub fn status_at(root: &Path) -> Status {
    std::fs::read(root.join("status.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
pub fn command(command: Command) -> Result<()> {
    let root = root()?;
    retype_learning::transport::private_directory(
        &root,
        &retype_learning::transport::user_sid().map_err(|_| "无法识别用户")?,
    )
    .map_err(|_| "无法保护跨设备目录")?;
    config::write_json(&root.join("command.json"), &command)?;
    crate::runtime::launch()
}
fn credential(id: &str) -> String {
    format!("lan-peer-{id}")
}
fn token(id: &str) -> String {
    retype_ai::secrets::cloud_password(&credential(id)).unwrap_or_default()
}
fn lan_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_link_local() || v.is_loopback(),
        IpAddr::V6(v) => v.is_loopback() || v.is_unique_local() || v.is_unicast_link_local(),
    }
}
fn tls() -> Result<(Arc<ServerConfig>, Vec<u8>)> {
    let mut saved =
        retype_ai::secrets::cloud_password("lan-tls-v1").map_err(|_| "无法读取设备身份")?;
    if saved.is_empty() {
        let cert = rcgen::generate_simple_self_signed(vec!["retype.local".into()])
            .map_err(|_| "无法生成设备身份")?;
        saved = json!({"cert":STANDARD.encode(cert.cert.der()), "key":STANDARD.encode(cert.signing_key.serialize_der())}).to_string();
        retype_ai::secrets::save_cloud_password("lan-tls-v1", &saved)
            .map_err(|_| "无法保存设备身份")?;
    }
    let data: Value = serde_json::from_str(&saved).map_err(|_| "设备身份损坏")?;
    let der = STANDARD
        .decode(data["cert"].as_str().unwrap_or_default())
        .map_err(|_| "设备证书损坏")?;
    let key = STANDARD
        .decode(data["key"].as_str().unwrap_or_default())
        .map_err(|_| "设备密钥损坏")?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| "TLS 配置失败")?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(der.clone())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
        )
        .map_err(|_| "设备证书无效")?;
    Ok((Arc::new(config), der))
}
fn send(stream: &mut StreamOwned<ServerConnection, TcpStream>, value: Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(&value).map_err(|_| "编码设备消息失败")?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .and_then(|_| stream.flush())
        .map_err(|_| "设备连接已断开".into())
}
fn broadcast(state: &mut State, clip: &Clip) {
    for (_, sender) in &state.senders {
        if let Ok(mut pending) = sender.lock() {
            *pending = Some(clip.clone());
        }
    }
}
fn session(
    socket: TcpStream,
    tls: Arc<ServerConfig>,
    cert: &[u8],
    shared: &Arc<Mutex<State>>,
    root: &Path,
    stop: &AtomicBool,
    clipboard: &Mailbox,
) -> Result<()> {
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|_| "无法设置连接超时")?;
    socket
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| "无法设置发送超时")?;
    socket.set_nodelay(true).map_err(|_| "无法设置连接")?;
    let connection = ServerConnection::new(tls).map_err(|_| "TLS 初始化失败")?;
    let mut stream = StreamOwned::new(connection, socket);
    let mut decoder = Decoder::default();
    let mut buffer = [0u8; 8192];
    let mut peer = String::new();
    let mut name = String::new();
    let mut client_confirmed = false;
    let mut authenticated = false;
    let mut pairing = false;
    let mut exchange = None;
    let mut server_proof = String::new();
    let mut pair_code = String::new();
    let pending = Arc::new(Mutex::new(None::<Clip>));
    let start = Instant::now();
    let mut heartbeat = Instant::now();
    let mut last_read = Instant::now();
    let mut guard = SessionGuard {
        state: shared,
        peer: None,
        pending: None,
    };
    while !stop.load(Ordering::Relaxed) {
        if !authenticated && start.elapsed() > Duration::from_secs(if pairing { 120 } else { 10 }) {
            return Err("关联超时".into());
        }
        if last_read.elapsed() > Duration::from_secs(45) {
            return Err("设备连接超时".into());
        }
        let count = match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                0
            }
            Err(_) => break,
        };
        if count > 0 {
            last_read = Instant::now();
        }
        for message in decoder.feed(&buffer[..count])? {
            match message["type"].as_str().unwrap_or_default() {
                "hello" if peer.is_empty() => {
                    peer = message["id"].as_str().unwrap_or_default().into();
                    name = message["name"]
                        .as_str()
                        .unwrap_or_default()
                        .chars()
                        .take(64)
                        .collect();
                    if message["version"] != 2 || !config::valid_id(&peer) || name.is_empty() {
                        return Err("设备协议不兼容".into());
                    }
                    let mut state = shared.lock().map_err(|_| "设备状态不可用")?;
                    if !state.config.enabled {
                        return Err("同步已关闭".into());
                    }
                    let saved = token(&peer);
                    if state.config.peers.iter().any(|p| p.id == peer)
                        && !saved.is_empty()
                        && message["token"].as_str() == Some(saved.as_str())
                    {
                        authenticated = true;
                    } else if state.pair_until.is_some_and(|t| t > Instant::now())
                        && state.status.pending.is_none()
                        && state.attempts < 5
                        && crate::pairing::valid_code(&state.pair_code)
                        && message["mode"].as_str().is_some_and(|mode| {
                            (mode == "show" && state.status.mode == "join")
                                || (mode == "join" && state.status.mode == "show")
                        })
                        && state.config.peers.len() < 8
                    {
                        state.attempts += 1;
                        pair_code = state.pair_code.clone();
                        let (start, outbound) = crate::pairing::begin(
                            &pair_code,
                            &peer,
                            &state.config.id,
                            &crate::hash(cert),
                        )?;
                        exchange = Some(start);
                        let request = uuid::Uuid::new_v4().simple().to_string();
                        state.status.pending = Some(Pending {
                            id: peer.clone(),
                            name: name.clone(),
                            session: request.clone(),
                        });
                        guard.pending = Some(request);
                        state.pending_until = Some(Instant::now() + Duration::from_secs(120));
                        pairing = true;
                        send(
                            &mut stream,
                            json!({"type":"pair", "id":state.config.id, "message":outbound}),
                        )?;
                    } else {
                        return Err("设备未关联或匹配码已失效".into());
                    }
                }
                "proof" if pairing && !client_confirmed => {
                    let proof = exchange
                        .take()
                        .ok_or("关联验证失败")?
                        .finish(message["message"].as_str().unwrap_or_default())?;
                    if let Err(error) =
                        proof.verify("client", message["proof"].as_str().unwrap_or_default())
                    {
                        if let Ok(mut state) = shared.lock() {
                            state.status.message = "匹配码不正确，请检查另一台设备显示的码".into();
                        }
                        send(
                            &mut stream,
                            json!({"type":"error", "message":"匹配码不正确"}),
                        )?;
                        return Err(error);
                    }
                    server_proof = proof.tag("server")?;
                    client_confirmed = true;
                }
                "clip" if authenticated => {
                    let clip: Clip = serde_json::from_value(message["clip"].clone())
                        .map_err(|_| "无效的文字消息")?;
                    if clip.origin != peer {
                        return Err("文字来源无效".into());
                    }
                    let mut state = shared.lock().map_err(|_| "设备状态不可用")?;
                    if state.journal.accept(clip.clone(), now()) {
                        if let Ok(mut incoming) = clipboard.lock() {
                            *incoming = Some(clip.clone());
                        }
                        broadcast(&mut state, &clip);
                    }
                }
                "ping" => {
                    send(&mut stream, json!({"type":"pong"}))?;
                }
                "pong" => {}
                _ => return Err("无效的设备消息".into()),
            }
        }
        if pairing && client_confirmed {
            let mut state = shared.lock().map_err(|_| "设备状态不可用")?;
            if state
                .status
                .pending
                .as_ref()
                .is_some_and(|p| p.id == peer && guard.pending.as_ref() == Some(&p.session))
                && state.pair_code == pair_code
                && state.pair_until.is_some_and(|t| t > Instant::now())
            {
                let secret = format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                );
                retype_ai::secrets::save_cloud_password(&credential(&peer), &secret)
                    .map_err(|_| "无法保存关联凭据")?;
                state.config.peers.retain(|p| p.id != peer);
                state.config.peers.push(Peer {
                    id: peer.clone(),
                    name: name.clone(),
                });
                config::write_json(&root.join("config.json"), &state.config)?;
                state.status.pending = None;
                state.pair_until = None;
                state.pending_until = None;
                state.pair_code.clear();
                state.status.code.clear();
                state.status.mode.clear();
                state.status.expires = 0;
                state.status.message = "关联成功".into();
                send(
                    &mut stream,
                    json!({"type":"paired", "token":secret, "id":state.config.id, "name":state.config.name, "proof":server_proof}),
                )?;
                authenticated = true;
                pairing = false;
            } else if state.status.pending.as_ref().is_none_or(|p| p.id != peer) {
                return Err("关联已取消".into());
            }
        }
        if authenticated {
            let mut state = shared.lock().map_err(|_| "设备状态不可用")?;
            if !state.config.enabled || !state.config.peers.iter().any(|p| p.id == peer) {
                break;
            }
            if guard.peer.is_none() {
                if state.status.online.contains(&peer) {
                    return Err("设备已有连接".into());
                }
                guard.peer = Some(peer.clone());
                state.status.online.push(peer.clone());
                state.senders.push((peer.clone(), Arc::clone(&pending)));
                send(
                    &mut stream,
                    json!({"type":"ready", "id":state.config.id, "name":state.config.name}),
                )?;
                if let Some(clip) = &state.journal.latest {
                    if clip.valid(now()) {
                        send(&mut stream, json!({"type":"clip", "clip":clip}))?;
                    }
                }
            }
            drop(state);
            let latest = pending.lock().map_err(|_| "设备消息不可用")?.take();
            if let Some(clip) = latest {
                if clip.valid(now()) {
                    send(&mut stream, json!({"type":"clip", "clip":clip}))?;
                }
            }
            if heartbeat.elapsed() >= Duration::from_secs(15) {
                send(&mut stream, json!({"type":"ping"}))?;
                heartbeat = Instant::now();
            }
        }
    }
    Ok(())
}

pub fn serve() -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let root = root()?;
    retype_learning::transport::private_directory(
        &root,
        &retype_learning::transport::user_sid().map_err(|_| "无法识别用户")?,
    )
    .map_err(|_| "无法保护跨设备目录")?;
    let _lock = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(root.join("host.lock"))
    {
        Ok(lock) => lock,
        Err(_) => return Ok(()),
    };
    let stop_file = root.join("stop");
    let _ = std::fs::remove_file(&stop_file);
    let cfg = load_at(&root)?;
    let shared = Arc::new(Mutex::new(State {
        config: cfg,
        status: Status::default(),
        pair_until: None,
        pending_until: None,
        journal: Journal::default(),
        seq: crate::runtime::now().saturating_mul(1000),
        senders: Vec::new(),
        pair_code: String::new(),
        attempts: 0,
    }));
    let stop = Arc::new(AtomicBool::new(false));
    let clip_tx: Mailbox = Arc::new(Mutex::new(None));
    let mut clip_rx = Some(Arc::clone(&clip_tx));
    let mut listener = None;
    let mut identity = None;
    let mut discovery: Option<mdns_sd::ServiceDaemon> = None;
    let mut watcher = None;
    let slots = Arc::new(AtomicUsize::new(0));
    let mut last_status = Instant::now() - Duration::from_secs(2);
    let mut advertised_mode = String::new();
    loop {
        if stop_file.exists() {
            stop.store(true, Ordering::Relaxed);
            break;
        }
        if let Some(active) = retype_learning::transport::read_machine_registry("ActiveDir") {
            if std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .is_some_and(|p| p != Path::new(&active))
            {
                stop.store(true, Ordering::Relaxed);
                drop(_lock);
                crate::runtime::launch()?;
                return Ok(());
            }
        }
        let command_path = root.join("command.json");
        if let Ok(bytes) = std::fs::read(&command_path) {
            let _ = std::fs::remove_file(&command_path);
            if let Ok(command) = serde_json::from_slice::<Command>(&bytes) {
                let mut s = shared.lock().map_err(|_| "设备状态不可用")?;
                match command {
                    Command::Enable(value) => {
                        s.config.enabled = value;
                        s.journal = Journal::default();
                        if !value {
                            s.status.pending = None;
                            s.pair_until = None;
                        }
                    }
                    Command::Pair => {
                        s.config.enabled = true;
                        s.pair_until = Some(Instant::now() + Duration::from_secs(120));
                        s.pair_code = crate::pairing::new_code();
                        s.status.code = s.pair_code.clone();
                        s.status.mode = "show".into();
                        s.status.expires = now() + 120_000;
                        s.attempts = 0;
                        s.status.pending = None;
                        s.status.message = "请在另一台设备输入此匹配码".into();
                    }
                    Command::Join(code) => {
                        if crate::pairing::valid_code(&code) {
                            s.config.enabled = true;
                            s.pair_until = Some(Instant::now() + Duration::from_secs(120));
                            s.pair_code = code;
                            s.status.code.clear();
                            s.status.mode = "join".into();
                            s.status.expires = now() + 120_000;
                            s.attempts = 0;
                            s.status.pending = None;
                            s.status.message = "正在查找并关联设备…".into();
                        }
                    }
                    Command::Cancel => {
                        s.pair_until = None;
                        s.pair_code.clear();
                        s.status.code.clear();
                        s.status.mode.clear();
                        s.status.expires = 0;
                        s.status.pending = None;
                        s.status.message = "关联已取消".into();
                    }
                    Command::Remove(id) => {
                        s.config.peers.retain(|p| p.id != id);
                        let _ = retype_ai::secrets::save_cloud_password(&credential(&id), "");
                    }
                }
                config::write_json(&root.join("config.json"), &s.config)?;
            }
        }
        let enabled = shared.lock().map_err(|_| "设备状态不可用")?.config.enabled;
        if !enabled && listener.is_some() {
            listener = None;
            identity = None;
            if let Some(mdns) = discovery.take() {
                let _ = mdns.shutdown();
            }
        }
        if enabled && listener.is_none() {
            let saved_port = shared.lock().map_err(|_| "设备状态不可用")?.config.port;
            let socket = TcpListener::bind(("0.0.0.0", saved_port))
                .or_else(|_| TcpListener::bind(("0.0.0.0", 0)))
                .map_err(|_| "无法启动局域网连接")?;
            socket.set_nonblocking(true).map_err(|_| "无法设置监听")?;
            let port = socket.local_addr().map_err(|_| "无法获取监听端口")?.port();
            identity = Some(tls()?);
            if let Ok(mdns) = mdns_sd::ServiceDaemon::new() {
                let _ = mdns.disable_interface(mdns_sd::IfKind::IPv6);
                let s = shared.lock().map_err(|_| "设备状态不可用")?;
                if let Ok(info) = mdns_sd::ServiceInfo::new(
                    "_retype-clip._tcp.local.",
                    &s.config.name,
                    &format!("retype-{}.local.", s.config.id),
                    "",
                    port,
                    [
                        ("id", s.config.id.as_str()),
                        ("version", "2"),
                        ("mode", s.status.mode.as_str()),
                    ]
                    .as_slice(),
                ) {
                    let _ = mdns.register(info.enable_addr_auto());
                }
                discovery = Some(mdns);
            }
            // Address inventory only; no data leaves these local interfaces.
            let mut s = shared.lock().map_err(|_| "设备状态不可用")?;
            s.status.port = port;
            s.config.port = port;
            config::write_json(&root.join("config.json"), &s.config)?;
            s.status.addresses = local_addresses(port);
            if s.status.mode.is_empty() {
                s.status.message = "等待已关联设备连接".into();
            }
            drop(s);
            listener = Some(socket);
            if let Some(rx) = clip_rx.take() {
                let quit = Arc::clone(&stop);
                let shared = Arc::clone(&shared);
                watcher = Some(std::thread::spawn(move || {
                    clipboard_loop(rx, &quit, &shared)
                }));
            }
            // The clipboard watcher starts once and survives disable/re-enable.
        }
        if let Some(mdns) = &discovery {
            let s = shared.lock().map_err(|_| "设备状态不可用")?;
            if advertised_mode != s.status.mode {
                if let Ok(info) = mdns_sd::ServiceInfo::new(
                    "_retype-clip._tcp.local.",
                    &s.config.name,
                    &format!("retype-{}.local.", s.config.id),
                    "",
                    s.status.port,
                    [
                        ("id", s.config.id.as_str()),
                        ("version", "2"),
                        ("mode", s.status.mode.as_str()),
                    ]
                    .as_slice(),
                ) {
                    let _ = mdns.register(info.enable_addr_auto());
                }
                advertised_mode = s.status.mode.clone();
            }
        }
        if let (Some(socket), Some((tls, cert))) = (&listener, &identity) {
            while let Ok((connection, address)) = socket.accept() {
                if !enabled || !lan_address(address.ip()) || slots.load(Ordering::Relaxed) >= 8 {
                    continue;
                }
                slots.fetch_add(1, Ordering::Relaxed);
                let slots = Arc::clone(&slots);
                let shared = Arc::clone(&shared);
                let quit = Arc::clone(&stop);
                let tls = Arc::clone(tls);
                let cert = cert.clone();
                let root = root.clone();
                let clipboard = Arc::clone(&clip_tx);
                std::thread::spawn(move || {
                    let _ = session(connection, tls, &cert, &shared, &root, &quit, &clipboard);
                    slots.fetch_sub(1, Ordering::Relaxed);
                });
            }
        }
        if last_status.elapsed() >= Duration::from_secs(1) {
            let mut s = shared.lock().map_err(|_| "设备状态不可用")?;
            if s.pending_until.is_some_and(|t| t <= Instant::now()) {
                s.status.pending = None;
                s.pending_until = None;
            }
            s.status.pairing = s.pair_until.is_some_and(|t| t > Instant::now());
            if (!s.status.pairing || (s.attempts >= 5 && s.status.pending.is_none()) || !enabled)
                && !s.pair_code.is_empty()
            {
                s.pair_code.clear();
                s.status.code.clear();
                s.status.mode.clear();
                s.status.expires = 0;
                s.status.pending = None;
                s.status.message = if s.attempts >= 5 {
                    "尝试次数过多，请重新生成匹配码"
                } else {
                    "匹配码已过期，请重试"
                }
                .into();
                s.pair_until = None;
            }
            if !enabled {
                s.status.message = "已关闭".into();
                s.status.online.clear();
            }
            config::write_json(&root.join("status.json"), &s.status)?;
            last_status = Instant::now();
        }
        if !enabled && !config::Config::load_at(&config::root()?)?.enabled && !command_path.exists()
        {
            stop.store(true, Ordering::Relaxed);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if let Some(mdns) = discovery {
        let _ = mdns.shutdown();
    }
    if let Some(thread) = watcher {
        let _ = thread.join();
    }
    Ok(())
}
fn local_addresses(port: u16) -> Vec<String> {
    let mut addresses: Vec<_> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|interface| interface.ip())
        .filter(|ip| ip.is_ipv4() && !ip.is_loopback() && lan_address(*ip))
        .map(|ip| format!("{ip}:{port}"))
        .collect();
    addresses.sort();
    addresses.dedup();
    addresses
}

#[allow(unsafe_code)]
fn clipboard_loop(incoming: Mailbox, stop: &AtomicBool, shared: &Mutex<State>) {
    use windows_sys::Win32::{
        Foundation::*,
        System::{DataExchange::*, LibraryLoader::GetModuleHandleW, Memory::*},
        UI::WindowsAndMessaging::*,
    };
    // SAFETY: This thread owns its message window and scopes clipboard access to
    // OpenClipboard/CloseClipboard; global allocations are bounded and locked.
    unsafe {
        let class = retype_learning::transport::wide("retype-lan-clipboard");
        let module = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: module,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            module,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        if AddClipboardFormatListener(hwnd) == 0 {
            DestroyWindow(hwnd);
            return;
        }
        let excluded = RegisterClipboardFormatW(
            retype_learning::transport::wide("ExcludeClipboardContentFromMonitorProcessing")
                .as_ptr(),
        );
        let mut own_sequence = GetClipboardSequenceNumber();
        let mut last_sequence = own_sequence;
        let mut pending_read: Option<(u32, Instant)> = None;
        let mut pending_write: Option<Clip> = None;
        while !stop.load(Ordering::Relaxed) {
            let mut message: MSG = std::mem::zeroed();
            while PeekMessageW(&mut message, hwnd, 0, 0, PM_REMOVE) != 0 {
                if message.message == WM_CLIPBOARDUPDATE {
                    let sequence = GetClipboardSequenceNumber();
                    if sequence != own_sequence && sequence != last_sequence {
                        if shared.lock().is_ok_and(|s| s.config.enabled) {
                            pending_read = Some((sequence, Instant::now()));
                        } else {
                            pending_read = None;
                        }
                    }
                    last_sequence = sequence;
                }
                DispatchMessageW(&message);
            }
            if let Some((sequence, started)) = pending_read {
                if started.elapsed() > Duration::from_secs(3) {
                    pending_read = None;
                    pending_write = None;
                } else if OpenClipboard(hwnd) != 0 {
                    if let Ok(mut state) = shared.lock() {
                        state.journal.latest = None;
                    }
                    if let Ok(mut pending) = incoming.lock() {
                        pending.take();
                    }
                    if sequence == GetClipboardSequenceNumber()
                        && IsClipboardFormatAvailable(excluded) == 0
                    {
                        let data = GetClipboardData(13);
                        if !data.is_null() {
                            let size = GlobalSize(data);
                            if size <= 2 * crate::clipboard_protocol::MAX_TEXT + 2 {
                                let ptr = GlobalLock(data).cast::<u16>();
                                if !ptr.is_null() {
                                    let slice = std::slice::from_raw_parts(ptr, size / 2);
                                    let end =
                                        slice.iter().position(|v| *v == 0).unwrap_or(slice.len());
                                    let text = String::from_utf16_lossy(&slice[..end]);
                                    if let Ok(mut state) = shared.lock() {
                                        if state.config.enabled {
                                            state.seq += 1;
                                            let seq = state.seq;
                                            let id = state.config.id.clone();
                                            if let Some(clip) =
                                                state.journal.local(&id, seq, text, now())
                                            {
                                                broadcast(&mut state, &clip);
                                            }
                                        }
                                    }
                                    GlobalUnlock(data);
                                }
                            }
                        }
                    }
                    CloseClipboard();
                    pending_read = None;
                    // Any local copy, including a non-text copy, takes precedence
                    // over a remote write that was waiting for this clipboard.
                    pending_write = None;
                }
            }
            if let Ok(mut incoming) = incoming.lock() {
                if let Some(clip) = incoming.take() {
                    pending_write = Some(clip);
                }
            }
            if let Some(clip) = &pending_write {
                let valid = shared
                    .lock()
                    .is_ok_and(|s| s.config.enabled && s.journal.latest.as_ref() == Some(clip))
                    && clip.valid(now());
                if !valid {
                    pending_write = None;
                } else if pending_read.is_none()
                    && GetClipboardSequenceNumber() == last_sequence
                    && OpenClipboard(hwnd) != 0
                {
                    let wide: Vec<u16> = clip.text.encode_utf16().chain(Some(0)).collect();
                    let data = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
                    let ptr = if data.is_null() {
                        std::ptr::null_mut()
                    } else {
                        GlobalLock(data).cast::<u16>()
                    };
                    let mut applied = false;
                    if !ptr.is_null() {
                        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                        GlobalUnlock(data);
                        if EmptyClipboard() != 0 && !SetClipboardData(13, data).is_null() {
                            applied = true;
                        } else {
                            GlobalFree(data);
                        }
                    } else if !data.is_null() {
                        GlobalFree(data);
                    }
                    CloseClipboard();
                    if applied {
                        own_sequence = GetClipboardSequenceNumber();
                        last_sequence = own_sequence;
                        pending_write = None;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        RemoveClipboardFormatListener(hwnd);
        DestroyWindow(hwnd);
        UnregisterClassW(class.as_ptr(), module);
    }
}
