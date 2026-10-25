//! Desktop clients connect to desktop hosts as well as accepting Android connections.
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
    let credentials = client_identity(remote);
    let mut remote = remote.to_owned();
    let secret = token(&remote);
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
    let pairing = !known || secret.is_empty();
    let resuming = !pairing && credentials["pin"].as_str().is_none_or(str::is_empty);
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    if pairing
        && (!crate::pairing::valid_code(&code) || pair_until.is_none_or(|t| t <= Instant::now()))
    {
        return Err("未开启关联".into());
    }
    let pin = if pairing || resuming {
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
        json!({"type":"hello","version":2,"id":id,"name":name,"mode":if pairing{&mode}else{""},"token":if pairing || resuming {""}else{&secret},"resume":if resuming {Some(&nonce)}else{None},"addresses":local_addresses(shared.lock().map_err(|_| "设备状态不可用")?.status.port)}),
    )?;
    let mut decoder = Decoder::default();
    let mut buffer = [0u8; 8192];
    let mut proof = None;
    let mut resumed = false;
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
                "resume" if resuming && !resumed && !ready => {
                    if message["id"] != remote {
                        return Err("设备身份不一致".into());
                    }
                    let proof = crate::pairing::Proof::resume(
                        &secret,
                        &id,
                        &remote,
                        &fingerprint,
                        &nonce,
                        message["challenge"].as_str().unwrap_or_default(),
                    )?;
                    proof
                        .verify("server", message["proof"].as_str().unwrap_or_default())
                        .map_err(|_| "设备身份验证失败")?;
                    send(
                        &mut stream,
                        json!({"type":"resumeProof","proof":proof.tag("client")?}),
                    )?;
                    resumed = true;
                    // The old shared secret authenticates the new transport role.
                    retype_ai::secrets::save_cloud_password(
                        &format!("lan-client-{remote}"),
                        &json!({"pin":fingerprint,"token":secret}).to_string(),
                    )
                    .map_err(|_| "无法保存设备身份")?;
                }
                "pair" if pairing && !paired => {
                    if remote.is_empty() {
                        remote = message["id"]
                            .as_str()
                            .filter(|s| config::valid_id(s) && *s != id)
                            .ok_or("设备身份无效")?
                            .into();
                    }
                    if message["id"] != remote {
                        return Err("设备身份不一致".into());
                    }
                    let (exchange, outbound) =
                        crate::pairing::begin(&code, &id, &remote, &fingerprint)?;
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
                        id: remote.clone(),
                        addresses: vec![address.to_string()],
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
                    diagnostic(root, "pairing_succeeded", json!({"peer":remote}));
                    s.status.message = "关联成功".into();
                    paired = true;
                }
                "ready" => {
                    if message["id"] != remote || (pairing && !paired) || (resuming && !resumed) {
                        return Err("设备未通过验证".into());
                    }
                    let mut s = shared.lock().map_err(|_| "设备状态不可用")?;
                    if !s.config.enabled || !s.config.peers.iter().any(|p| p.id == remote) {
                        return Err("设备已解除关联".into());
                    }
                    if s.status.online.contains(&remote) {
                        return Err("设备已有连接".into());
                    }
                    let mut endpoints = advertised_addresses(&message["addresses"]);
                    endpoints.push(address.to_string());
                    if pairing {
                        if let Some(target) = s.target.take() {
                            endpoints.push(target);
                        }
                    }
                    if remember_addresses(&mut s.config, &remote, endpoints) {
                        config::write_json(&root.join("config.json"), &s.config)?;
                    }
                    s.status
                        .connections
                        .insert(remote.clone(), address.to_string());
                    diagnostic(
                        root,
                        "connection_ready",
                        json!({"peer":remote,"address":address.to_string()}),
                    );
                    ready = true;
                    guard.peer = Some(remote.clone());
                    s.status.online.push(remote.clone());
                    s.senders.push((remote.clone(), Arc::clone(&outgoing)));
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
    if ready || stop.load(Ordering::Relaxed) {
        Ok(())
    } else {
        Err("设备在完成验证前断开连接".into())
    }
}
pub(super) fn connect_candidates(
    endpoints: &[String],
    remote: &str,
    shared: &Arc<Mutex<State>>,
    stop: &AtomicBool,
    clipboard: &Mailbox,
    root: &Path,
) -> Result<()> {
    let mut endpoints = endpoints.to_vec();
    endpoints.sort_by_key(|s| {
        s.parse::<std::net::SocketAddr>().map_or(1, |a| {
            if address::is_tailscale(a.ip()) {
                2
            } else {
                0
            }
        })
    });
    let mut last_error = "设备离线".to_string();
    for endpoint in endpoints.into_iter().take(8) {
        if stop.load(Ordering::Relaxed)
            || shared
                .lock()
                .is_ok_and(|s| !s.config.enabled || s.status.online.iter().any(|id| id == remote))
        {
            return Ok(());
        }
        let addresses = match address::resolve(&endpoint) {
            Ok(addresses) => addresses,
            Err(error) => {
                diagnostic(
                    root,
                    "resolve_error",
                    json!({"peer":remote,"endpoint":endpoint,"error":error}),
                );
                last_error = error;
                continue;
            }
        };
        for address in addresses {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            diagnostic(
                root,
                "connection_started",
                json!({"peer":remote,"endpoint":endpoint,"address":address.to_string()}),
            );
            match connect(address, remote, shared, stop, clipboard, root) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    diagnostic(
                        root,
                        "connection_ended",
                        json!({"peer":remote,"address":address.to_string(),"error":error}),
                    );
                    if let Ok(mut s) = shared.lock() {
                        if s.status.online.is_empty() {
                            s.status.message = error.clone();
                        }
                    }
                    last_error = error;
                }
            }
        }
    }
    Err(last_error)
}
pub(super) fn watch(
    shared: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    clipboard: Mailbox,
    root: PathBuf,
) {
    let mdns = mdns_sd::ServiceDaemon::new().ok();
    let mut events = None;
    let active = Arc::new(Mutex::new(std::collections::BTreeSet::<String>::new()));
    let mut nearby = std::collections::BTreeMap::<String, Box<mdns_sd::ResolvedService>>::new();
    let retry = Arc::new(Mutex::new(std::collections::BTreeMap::<
        String,
        (Instant, u32),
    >::new()));
    let mut scan = Instant::now() - Duration::from_secs(3);
    let mut last_code = String::new();
    let mut last_routes = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let enabled = shared.lock().is_ok_and(|s| s.config.enabled);
        if !enabled {
            if events.take().is_some() {
                if let Some(mdns) = &mdns {
                    let _ = mdns.stop_browse("_retype-clip._tcp.local.");
                }
            }
            nearby.clear();
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        if events.is_none() {
            if let Some(mdns) = &mdns {
                events = mdns.browse("_retype-clip._tcp.local.").ok();
                diagnostic(
                    &root,
                    "discovery_started",
                    json!({"success":events.is_some()}),
                );
            }
        }
        if let Some(receiver) = &events {
            match receiver.recv_timeout(Duration::from_millis(200)) {
                Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                    let id = info.get_property_val_str("id").unwrap_or("").to_owned();
                    if config::valid_id(&id) && (nearby.contains_key(&id) || nearby.len() < 64) {
                        diagnostic(
                            &root,
                            "device_discovered",
                            json!({"peer":id,"port":info.get_port()}),
                        );
                        nearby.insert(id, info);
                    }
                }
                Ok(mdns_sd::ServiceEvent::ServiceRemoved(_, name)) => {
                    nearby.retain(|_, i| i.get_fullname() != name)
                }
                Err(mdns_sd::RecvTimeoutError::Disconnected) => {
                    events = None;
                    nearby.clear();
                }
                _ => {}
            }
        } else {
            std::thread::sleep(Duration::from_millis(200));
        }
        if scan.elapsed() < Duration::from_secs(3) {
            continue;
        }
        scan = Instant::now();
        let Ok(s) = shared.lock() else { continue };
        let routes: Vec<_> = s
            .config
            .peers
            .iter()
            .map(|p| (p.id.clone(), p.addresses.clone()))
            .collect();
        if last_code != s.pair_code || last_routes != routes {
            last_routes = routes;
            if let Ok(mut r) = retry.lock() {
                r.clear();
            }
            last_code = s.pair_code.clone();
        }
        let mut targets = std::collections::BTreeMap::<String, (String, Vec<String>)>::new();
        for peer in &s.config.peers {
            if !peer.addresses.is_empty() {
                targets.insert(peer.id.clone(), (peer.id.clone(), peer.addresses.clone()));
            }
        }
        let pairing = s.pair_until.is_some_and(|t| t > Instant::now());
        for (id, info) in &nearby {
            let opposite = (s.status.mode == "show"
                && info.get_property_val_str("mode") == Some("join"))
                || (s.status.mode == "join" && info.get_property_val_str("mode") == Some("show"));
            let known = s.config.peers.iter().any(|p| &p.id == id);
            if !known
                && (!pairing
                    || !opposite
                    || s.target.is_some()
                    || (info.get_property_val_str("dial") == Some("1") && s.config.id > *id))
            {
                continue;
            }
            if id == &s.config.id {
                continue;
            }
            let entry = targets
                .entry(id.clone())
                .or_insert_with(|| (id.clone(), Vec::new()));
            for ip in info.get_addresses_v4() {
                if lan_address(IpAddr::V4(ip)) {
                    let a = format!("{ip}:{}", info.get_port());
                    if !entry.1.contains(&a) {
                        entry.1.push(a);
                    }
                }
            }
        }
        if pairing {
            if let Some(target) = &s.target {
                targets.insert(
                    format!("pair:{target}"),
                    (String::new(), vec![target.clone()]),
                );
            }
        }
        let local_id = s.config.id.clone();
        let online = s.status.online.clone();
        drop(s);
        for (key, (id, addresses)) in targets {
            if online.contains(&id) || addresses.is_empty() {
                continue;
            }
            if retry
                .lock()
                .is_ok_and(|r| r.get(&key).is_some_and(|(at, _)| *at > Instant::now()))
            {
                continue;
            }
            let Ok(mut busy) = active.lock() else {
                continue;
            };
            if busy.len() >= 8 || !busy.insert(key.clone()) {
                continue;
            }
            drop(busy);
            let state = Arc::clone(&shared);
            let quit = Arc::clone(&stop);
            let clips = Arc::clone(&clipboard);
            let folder = root.clone();
            let active = Arc::clone(&active);
            let retry = Arc::clone(&retry);
            let local_id = local_id.clone();
            std::thread::spawn(move || {
                // Stagger simultaneous dials while still allowing either reachable direction.
                if !id.is_empty() && local_id > id {
                    std::thread::sleep(Duration::from_millis(1500));
                }
                let success =
                    connect_candidates(&addresses, &id, &state, &quit, &clips, &folder).is_ok();
                if let Ok(mut r) = retry.lock() {
                    let n = if success {
                        0
                    } else {
                        r.get(&key).map_or(0, |(_, n)| *n).saturating_add(1)
                    };
                    let seconds = if success {
                        1
                    } else {
                        (3u64.saturating_mul(1u64 << n.min(4))).min(30)
                    };
                    r.insert(
                        key.clone(),
                        (Instant::now() + Duration::from_secs(seconds), n),
                    );
                }
                if let Ok(mut busy) = active.lock() {
                    busy.remove(&key);
                }
            });
        }
    }
    if let Some(mdns) = mdns {
        let _ = mdns.shutdown();
    }
}
