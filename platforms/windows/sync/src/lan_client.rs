//! macOS connects to desktop hosts as well as accepting Android connections.
//! TLS certificates are pinned after certificate-bound SPAKE2 confirmation.
use super::*;
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};

#[derive(Debug)]
struct Pin(String);
impl ServerCertVerifier for Pin {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        if !self.0.is_empty() && crate::hash(end.as_ref()) != self.0 {
            return Err(rustls::Error::General("Device certificate changed".into()));
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            signature,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            signature,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
fn send(stream: &mut StreamOwned<rustls::ClientConnection, TcpStream>, value: Value) -> Result<()> {
    let mut frame = serde_json::to_vec(&value).map_err(|_| "消息无效")?;
    frame.push(b'\n');
    stream
        .write_all(&frame)
        .and_then(|_| stream.flush())
        .map_err(|_| "设备连接已断开".into())
}
pub(super) fn connect(
    address: std::net::SocketAddr,
    remote: &str,
    shared: &Arc<Mutex<State>>,
    stop: &AtomicBool,
    clipboard: &Mailbox,
    root: &Path,
) -> Result<()> {
    if !lan_address(address.ip()) {
        return Err("仅支持局域网地址".into());
    }
    let saved =
        retype_ai::secrets::cloud_password(&format!("lan-client-{remote}")).unwrap_or_default();
    let credentials: Value = serde_json::from_str(&saved).unwrap_or(Value::Null);
    let (id, name, mode, code, pair_until, known) = {
        let s = shared.lock().map_err(|_| "设备状态不可用")?;
        (
            s.config.id.clone(),
            s.config.name.clone(),
            s.status.mode.clone(),
            s.pair_code.clone(),
            s.pair_until,
            s.config.peers.iter().any(|p| p.id == remote),
        )
    };
    let pairing = !known || credentials["token"].as_str().is_none_or(str::is_empty);
    if pairing
        && (!crate::pairing::valid_code(&code) || pair_until.is_none_or(|t| t <= Instant::now()))
    {
        return Err("未开启关联".into());
    }
    let pin = if pairing {
        String::new()
    } else {
        credentials["pin"].as_str().ok_or("设备身份丢失")?.into()
    };
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| "TLS初始化失败")?
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(Pin(pin)))
    .with_no_client_auth();
    let socket =
        TcpStream::connect_timeout(&address, Duration::from_secs(3)).map_err(|_| "设备连接失败")?;
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|_| "无法设置连接")?;
    socket
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| "无法设置连接")?;
    let _ = socket.set_nodelay(true);
    let connection = rustls::ClientConnection::new(
        Arc::new(tls),
        ServerName::try_from("retype.local").map_err(|_| "设备名称无效")?,
    )
    .map_err(|_| "TLS初始化失败")?;
    let mut stream = StreamOwned::new(connection, socket);
    let start = Instant::now();
    while stream.conn.is_handshaking() {
        if start.elapsed() > Duration::from_secs(5) {
            return Err("设备连接超时".into());
        }
        match stream.conn.complete_io(&mut stream.sock) {
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(_) => return Err("设备身份验证失败".into()),
        }
    }
    let cert = stream
        .conn
        .peer_certificates()
        .and_then(|c| c.first())
        .ok_or("缺少设备证书")?;
    let fingerprint = crate::hash(cert.as_ref());
    send(
        &mut stream,
        json!({"type":"hello","version":2,"id":id,"name":name,"mode":if pairing{&mode}else{""},"token":if pairing{""}else{credentials["token"].as_str().unwrap_or("")}}),
    )?;
    let mut decoder = Decoder::default();
    let mut buffer = [0u8; 8192];
    let mut proof = None;
    let mut paired = false;
    let mut ready = false;
    let mut last_read = Instant::now();
    let mut heartbeat = Instant::now();
    let outgoing = Arc::new(Mutex::new(None::<Clip>));
    let mut guard = SessionGuard {
        state: shared,
        peer: None,
        pending: None,
    };
    while !stop.load(Ordering::Relaxed) {
        {
            let s = shared.lock().map_err(|_| "设备状态不可用")?;
            if !s.config.enabled || (ready && !s.config.peers.iter().any(|p| p.id == remote)) {
                break;
            }
        }
        if !ready && start.elapsed() > Duration::from_secs(if pairing { 120 } else { 10 }) {
            return Err("关联超时".into());
        }
        if last_read.elapsed() > Duration::from_secs(45) {
            return Err("设备连接超时".into());
        }
        if ready {
            if let Some(clip) = outgoing.lock().map_err(|_| "消息状态无效")?.take() {
                if clip.origin == id && clip.valid(now()) {
                    send(&mut stream, json!({"type":"clip","clip":clip}))?;
                }
            }
            if heartbeat.elapsed() >= Duration::from_secs(15) {
                send(&mut stream, json!({"type":"ping"}))?;
                heartbeat = Instant::now();
            }
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
                "pair" if pairing && !paired => {
                    if message["id"] != remote {
                        return Err("设备身份不一致".into());
                    }
                    let (exchange, outbound) =
                        crate::pairing::begin(&code, &id, remote, &fingerprint)?;
                    let confirmation =
                        exchange.finish(message["message"].as_str().ok_or("验证数据无效")?)?;
                    send(
                        &mut stream,
                        json!({"type":"proof","message":outbound,"proof":confirmation.tag("client")?}),
                    )?;
                    proof = Some(confirmation);
                }
                "paired" if pairing => {
                    let confirmation = proof.take().ok_or("缺少设备验证")?;
                    confirmation.verify("server", message["proof"].as_str().unwrap_or(""))?;
                    let token = message["token"]
                        .as_str()
                        .filter(|t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()))
                        .ok_or("设备凭据无效")?;
                    let mut s = shared.lock().map_err(|_| "设备状态不可用")?;
                    if message["id"] != remote
                        || s.pair_code != code
                        || s.pair_until.is_none_or(|t| t <= Instant::now())
                    {
                        return Err("关联已取消".into());
                    }
                    retype_ai::secrets::save_cloud_password(
                        &format!("lan-client-{remote}"),
                        &json!({"pin":fingerprint,"token":token}).to_string(),
                    )
                    .map_err(|_| "无法保存关联凭据")?;
                    s.config.peers.retain(|p| p.id != remote);
                    s.config.peers.push(Peer {
                        id: remote.into(),
                        name: message["name"]
                            .as_str()
                            .unwrap_or("Desktop")
                            .chars()
                            .take(64)
                            .collect(),
                    });
                    config::write_json(&root.join("config.json"), &s.config)?;
                    s.pair_until = None;
                    s.pair_code.clear();
                    s.status.code.clear();
                    s.status.mode.clear();
                    s.status.message = "关联成功".into();
                    paired = true;
                }
                "ready" => {
                    if message["id"] != remote || (pairing && !paired) {
                        return Err("设备未通过验证".into());
                    }
                    let mut s = shared.lock().map_err(|_| "设备状态不可用")?;
                    if s.status.online.iter().any(|p| p == remote) {
                        return Err("设备已有连接".into());
                    }
                    ready = true;
                    guard.peer = Some(remote.into());
                    s.status.online.push(remote.into());
                    s.senders.push((remote.into(), Arc::clone(&outgoing)));
                    s.status.message = "已连接".into();
                    if let Some(clip) = &s.journal.latest {
                        if clip.origin == id && clip.valid(now()) {
                            send(&mut stream, json!({"type":"clip","clip":clip}))?;
                        }
                    }
                }
                "clip" if ready => {
                    let clip: Clip = serde_json::from_value(message["clip"].clone())
                        .map_err(|_| "文字消息无效")?;
                    let mut s = shared.lock().map_err(|_| "设备状态不可用")?;
                    if s.journal.accept(clip.clone(), now()) {
                        if let Ok(mut incoming) = clipboard.lock() {
                            *incoming = Some(clip.clone());
                        }
                        broadcast(&mut s, &clip);
                    }
                }
                "ping" => send(&mut stream, json!({"type":"pong"}))?,
                "pong" => {}
                "error" => return Err("匹配码不正确或设备拒绝关联".into()),
                _ => return Err("设备消息无效".into()),
            }
        }
    }
    Ok(())
}
#[cfg(target_os = "macos")]
pub(super) fn watch(
    shared: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    clipboard: Mailbox,
    root: PathBuf,
) {
    let Ok(mdns) = mdns_sd::ServiceDaemon::new() else {
        return;
    };
    let mut events = None;
    let active = Arc::new(Mutex::new(std::collections::BTreeSet::<String>::new()));
    let mut nearby = std::collections::BTreeMap::new();
    let mut scan = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let enabled = shared.lock().is_ok_and(|s| s.config.enabled);
        if !enabled {
            if events.take().is_some() {
                let _ = mdns.stop_browse("_retype-clip._tcp.local.");
            }
            nearby.clear();
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        if events.is_none() {
            events = mdns.browse("_retype-clip._tcp.local.").ok();
        }
        let Some(receiver) = events.as_ref() else {
            std::thread::sleep(Duration::from_secs(3));
            continue;
        };
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                let id = info.get_property_val_str("id").unwrap_or("").to_owned();
                if config::valid_id(&id) && (nearby.contains_key(&id) || nearby.len() < 64) {
                    nearby.insert(id, info);
                }
            }
            Ok(mdns_sd::ServiceEvent::ServiceRemoved(_, name)) => {
                nearby.retain(|_, info| info.get_fullname() != name);
            }
            _ => {}
        }
        if scan.elapsed() < Duration::from_secs(3) {
            continue;
        }
        scan = Instant::now();
        for (remote, info) in &nearby {
            let Ok(s) = shared.lock() else { break };
            let peer_mode = info.get_property_val_str("mode").unwrap_or("");
            let pair = s.pair_until.is_some_and(|t| t > Instant::now())
                && ((s.status.mode == "show" && peer_mode == "join")
                    || (s.status.mode == "join" && peer_mode == "show"));
            if !s.config.enabled
                || remote == &s.config.id
                || s.status.online.contains(remote)
                || (!pair && !s.config.peers.iter().any(|p| &p.id == remote))
                || (info.get_property_val_str("platform") == Some("macos") && s.config.id > *remote)
            {
                continue;
            }
            let id = remote.clone();
            drop(s);
            let Ok(mut connecting) = active.lock() else {
                break;
            };
            if !connecting.insert(id.clone()) {
                continue;
            }
            drop(connecting);
            let Some(ip) = info
                .get_addresses_v4()
                .into_iter()
                .find(|ip| lan_address(IpAddr::V4(*ip)))
            else {
                if let Ok(mut a) = active.lock() {
                    a.remove(&id);
                }
                continue;
            };
            let address = std::net::SocketAddr::new(IpAddr::V4(ip), info.get_port());
            let state = Arc::clone(&shared);
            let quit = Arc::clone(&stop);
            let clips = Arc::clone(&clipboard);
            let folder = root.clone();
            let active = Arc::clone(&active);
            std::thread::spawn(move || {
                let result = connect(address, &id, &state, &quit, &clips, &folder);
                if let Ok(mut a) = active.lock() {
                    a.remove(&id);
                }
                if let Err(message) = result {
                    if let Ok(mut s) = state.lock() {
                        if pair {
                            s.pair_until = None;
                            s.status.message = message;
                        }
                    }
                }
            });
        }
    }
    let _ = mdns.shutdown();
}
