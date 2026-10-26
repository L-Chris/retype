use super::*;
use std::io::{BufRead, BufReader};
type Client = BufReader<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>;

#[test]
fn idle_accepted_socket_waits_for_input_instead_of_spinning() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut client = TcpStream::connect(listener.local_addr().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    // TCP connect completion can precede the listener becoming readable on macOS.
    let deadline = Instant::now() + Duration::from_secs(2);
    let (mut accepted, _) = loop {
        match listener.accept() {
            Ok(connection) => break connection,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(error) => return Err(format!("accept failed: {error}")),
        }
    };
    // Winsock inherits the listener's nonblocking mode. Force that precondition
    // on every platform so macOS tests cover the Windows regression too.
    accepted.set_nonblocking(true).map_err(|e| e.to_string())?;
    configure_session_socket(&accepted)?;
    let start = Instant::now();
    let error = match accepted.read(&mut [0; 1]) {
        Err(error) => error,
        Ok(_) => return Err("peer has sent no data".into()),
    };
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    assert!(
        start.elapsed() >= Duration::from_millis(100),
        "idle read returned immediately; the session would spin"
    );
    // A real message still wakes the read immediately; no unconditional sleep
    // is added to the session's clipboard delivery path.
    client.write_all(b"x").map_err(|e| e.to_string())?;
    let mut byte = [0; 1];
    accepted.read_exact(&mut byte).map_err(|e| e.to_string())?;
    assert_eq!(&byte, b"x");
    Ok(())
}

// Standalone Rust tests have no AppKit host to register Keychain callbacks.
// Keep synthetic credentials in memory; never touch the user's Keychain.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod test_secrets {
    use std::{
        collections::BTreeMap,
        ffi::{c_char, CStr},
        sync::{Mutex, OnceLock},
    };
    fn store() -> &'static Mutex<BTreeMap<String, String>> {
        static STORE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
        STORE.get_or_init(Default::default)
    }
    unsafe extern "C" fn read(account: *const c_char, output: *mut u8, capacity: usize) -> isize {
        let Ok(values) = store().lock() else {
            return -1;
        };
        let name = CStr::from_ptr(account).to_string_lossy();
        let value = values
            .get(name.as_ref())
            .map(String::as_bytes)
            .unwrap_or_default();
        if value.len() > capacity {
            return -1;
        }
        std::ptr::copy_nonoverlapping(value.as_ptr(), output, value.len());
        value.len() as isize
    }
    unsafe extern "C" fn save(account: *const c_char, value: *const c_char) -> bool {
        let Ok(mut values) = store().lock() else {
            return false;
        };
        let name = CStr::from_ptr(account).to_string_lossy().into_owned();
        let value = CStr::from_ptr(value).to_string_lossy().into_owned();
        if value.is_empty() {
            values.remove(&name);
        } else {
            values.insert(name, value);
        }
        true
    }
    pub fn register() {
        retype_ai::secrets::register(read, save);
    }
}
struct Fixture {
    address: std::net::SocketAddr,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    tls: Arc<ServerConfig>,
    cert: Vec<u8>,
    root: PathBuf,
    incoming: Mailbox,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Fixture {
    fn new() -> Result<Self> {
        #[cfg(target_os = "macos")]
        test_secrets::register();
        let certificate = rcgen::generate_simple_self_signed(vec!["retype.local".into()])
            .map_err(|e| e.to_string())?;
        let cert = certificate.cert.der().to_vec();
        let tls = Arc::new(
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_no_client_auth()
                .with_single_cert(
                    vec![CertificateDer::from(cert.clone())],
                    PrivatePkcs8KeyDer::from(certificate.signing_key.serialize_der()).into(),
                )
                .map_err(|e| e.to_string())?,
        );
        let root =
            std::env::temp_dir().join(format!("retype-lan-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let cfg = Config {
            enabled: true,
            name: "LAN test computer".into(),
            ..Default::default()
        };
        let state = Arc::new(Mutex::new(State {
            config: cfg,
            status: Status {
                mode: "show".into(),
                ..Default::default()
            },
            pair_until: Some(Instant::now() + Duration::from_secs(120)),
            pending_until: None,
            journal: Journal::default(),
            seq: 1,
            senders: Vec::new(),
            pair_code: "123456".into(),
            attempts: 0,
            target: None,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        let incoming: Mailbox = Arc::new(Mutex::new(None));
        let tx = Arc::clone(&incoming);
        let s = Arc::clone(&state);
        let quit = Arc::clone(&stop);
        let identity = Arc::clone(&tls);
        let der = cert.clone();
        let folder = root.clone();
        let worker = std::thread::spawn(move || {
            let mut sessions = Vec::new();
            while !quit.load(Ordering::Relaxed) {
                if let Ok((socket, _)) = listener.accept() {
                    let s = Arc::clone(&s);
                    let quit = Arc::clone(&quit);
                    let tls = Arc::clone(&identity);
                    let der = der.clone();
                    let folder = folder.clone();
                    let tx = Arc::clone(&tx);
                    sessions.push(std::thread::spawn(move || {
                        let _ = session(socket, tls, &der, &s, &folder, &quit, &tx);
                    }));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            for worker in sessions {
                let _ = worker.join();
            }
        });
        Ok(Self {
            address,
            state,
            stop,
            tls,
            cert,
            root,
            incoming,
            worker: Some(worker),
        })
    }
    fn client(&self) -> Result<Client> {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from(self.cert.clone()))
            .map_err(|e| e.to_string())?;
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let connection = rustls::ClientConnection::new(
            Arc::new(config),
            rustls::pki_types::ServerName::try_from("retype.local").map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let socket = TcpStream::connect(self.address).map_err(|e| e.to_string())?;
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        Ok(BufReader::new(rustls::StreamOwned::new(connection, socket)))
    }
    fn receive(&self, timeout: Duration) -> Result<Clip> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(clip) = self.incoming.lock().map_err(|_| "lock")?.take() {
                return Ok(clip);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err("no clipboard event".into())
    }
    fn publish_image(&self) -> Result<String> {
        self.publish_image_bytes(include_bytes!("../../../../assets/logo.png"))
    }
    fn publish_image_bytes(&self, bytes: &[u8]) -> Result<String> {
        let mut s = self.state.lock().map_err(|_| "lock")?;
        s.seq += 1;
        let seq = s.seq;
        let id = s.config.id.clone();
        let clip = s
            .journal
            .local_image(&id, seq, bytes, now())
            .ok_or("invalid image")?;
        let encoded = clip.image.clone().ok_or("missing PNG")?;
        broadcast(&mut s, &clip);
        Ok(encoded)
    }
    fn publish(&self, text: &str) -> Result<()> {
        let mut s = self.state.lock().map_err(|_| "lock")?;
        s.seq += 1;
        let seq = s.seq;
        let id = s.config.id.clone();
        if let Some(clip) = s.journal.local(&id, seq, text.into(), now()) {
            broadcast(&mut s, &clip);
        }
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Ok(state) = self.state.lock() {
            for peer in &state.config.peers {
                let _ = retype_ai::secrets::save_cloud_password(&credential(&peer.id), "");
                let _ =
                    retype_ai::secrets::save_cloud_password(&format!("lan-client-{}", peer.id), "");
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn desktop_client_pairs_with_certificate_proofs_and_synchronizes_both_directions() -> Result<()> {
    let server = Fixture::new()?;
    let client = Fixture::new()?;
    client.state.lock().map_err(|_| "lock")?.status.mode = "join".into();
    let remote = server.state.lock().map_err(|_| "lock")?.config.id.clone();
    let state = Arc::clone(&client.state);
    let stop = Arc::clone(&client.stop);
    let incoming = Arc::clone(&client.incoming);
    let folder = client.root.clone();
    let id = remote.clone();
    let address = server.address;
    let worker = std::thread::spawn(move || {
        outbound::connect(address, &id, &state, &stop, &incoming, &folder)
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    while !client
        .state
        .lock()
        .map_err(|_| "lock")?
        .status
        .online
        .contains(&remote)
    {
        if Instant::now() > deadline {
            client.stop.store(true, Ordering::Relaxed);
            let result = worker.join().map_err(|_| "client worker")?;
            return Err(format!("desktop client did not pair: {result:?}"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    client.publish("Mac → desktop 中文")?;
    assert_eq!(
        server.receive(Duration::from_secs(3))?.text,
        "Mac → desktop 中文"
    );
    server.publish("desktop → Mac 👋")?;
    assert_eq!(
        client.receive(Duration::from_secs(3))?.text,
        "desktop → Mac 👋"
    );
    let image = client.publish_image()?;
    assert_eq!(
        server.receive(Duration::from_secs(3))?.image.as_deref(),
        Some(image.as_str())
    );
    let image = server.publish_image()?;
    assert_eq!(
        client.receive(Duration::from_secs(3))?.image.as_deref(),
        Some(image.as_str())
    );
    let saved = retype_ai::secrets::cloud_password(&format!("lan-client-{remote}"))
        .map_err(|e| e.to_string())?;
    let identity: Value = serde_json::from_str(&saved).map_err(|e| e.to_string())?;
    assert_eq!(identity["pin"], crate::hash(&server.cert));
    assert_eq!(identity["token"].as_str().map(str::len), Some(64));
    client.stop.store(true, Ordering::Relaxed);
    worker.join().map_err(|_| "client worker")??;
    Ok(())
}
fn write(client: &mut Client, message: Value) -> Result<()> {
    client
        .get_mut()
        .write_all(format!("{message}\n").as_bytes())
        .map_err(|e| e.to_string())
}
fn read(client: &mut Client) -> Result<Value> {
    let mut line = String::new();
    client.read_line(&mut line).map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|e| e.to_string())
}

#[test]
fn tls_pairing_bidirectional_copy_deduplication_and_reconnect() -> Result<()> {
    let fixture = Fixture::new()?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut client = fixture.client()?;
    write(
        &mut client,
        json!({"type":"hello","version":2,"id":id,"name":"Test phone","mode":"join","token":""}),
    )?;
    let pair = read(&mut client)?;
    let (exchange, message) = crate::pairing::begin(
        "123456",
        &id,
        pair["id"].as_str().ok_or("id")?,
        &crate::hash(&fixture.cert),
    )?;
    let proof = exchange.finish(pair["message"].as_str().ok_or("message")?)?;
    write(
        &mut client,
        json!({"type":"proof","message":message,"proof":proof.tag("client")?}),
    )?;
    let paired = read(&mut client)?;
    assert_eq!(paired["type"], "paired");
    proof.verify("server", paired["proof"].as_str().ok_or("proof")?)?;
    let secret = paired["token"].as_str().ok_or("token")?.to_string();
    assert_eq!(read(&mut client)?["type"], "ready");
    // This legacy client did not advertise images. It must keep its text session.
    fixture.publish_image()?;
    std::thread::sleep(Duration::from_millis(300));
    fixture.publish("电脑\nHello 👋")?;
    let from_pc = read(&mut client)?;
    assert_eq!(from_pc["clip"]["text"], "电脑\nHello 👋");
    let clip = Clip {
        origin: id.clone(),
        seq: 1,
        clock: now() + 100,
        created: now(),
        text: "手机\nworld".into(),
        image: None,
    };
    write(&mut client, json!({"type":"clip", "clip":clip}))?;
    assert_eq!(fixture.receive(Duration::from_secs(3))?.text, "手机\nworld");
    assert_eq!(read(&mut client)?["clip"]["text"], "手机\nworld");
    write(&mut client, json!({"type":"clip", "clip":clip}))?;
    assert!(fixture.receive(Duration::from_millis(300)).is_err());
    drop(client);
    let until = Instant::now() + Duration::from_secs(3);
    while !fixture
        .state
        .lock()
        .map_err(|_| "lock")?
        .status
        .online
        .is_empty()
        && Instant::now() < until
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut client = fixture.client()?;
    write(
        &mut client,
        json!({"type":"hello", "version":2, "id":id, "name":"Test phone", "token":secret}),
    )?;
    assert_eq!(read(&mut client)?["type"], "ready");
    assert_eq!(read(&mut client)?["clip"]["text"], "手机\nworld");
    // Certificate trust is explicit and no plaintext text or token is persisted.
    let config =
        std::fs::read_to_string(fixture.root.join("config.json")).map_err(|e| e.to_string())?;
    assert!(!config.contains(&secret));
    assert!(!config.contains("world"));
    fixture
        .state
        .lock()
        .map_err(|_| "lock")?
        .config
        .peers
        .clear();
    assert!(read(&mut client).is_err());
    // Revoked credentials are removed even though the peer is no longer in config.
    retype_ai::secrets::save_cloud_password(&credential(&id), "").map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
#[ignore = "opt-in Android emulator integration; starts a synthetic LAN endpoint for 60 seconds"]
fn android_emulator_bidirectional_clipboard() -> Result<()> {
    let fixture = Fixture::new()?;
    if std::env::var_os("RETYPE_LAN_TEST_PHONE_SHOW").is_some() {
        fixture.state.lock().map_err(|_| "lock")?.status.mode = "join".into();
    }
    let _identity = &fixture.tls;
    let path = std::env::temp_dir().join("retype-lan-emulator-port.txt");
    std::fs::write(&path, fixture.address.port().to_string()).map_err(|e| e.to_string())?;
    println!(
        "Synthetic LAN endpoint ready at port {}",
        fixture.address.port()
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut published = false;
    let mut text_received = false;
    let mut originals = Vec::new();
    for bytes in [
        include_bytes!("../../../../core/sync/tests/images/sample.jpg").as_slice(),
        include_bytes!("../../../../core/sync/tests/images/sample.webp").as_slice(),
    ] {
        originals.push(bytes.to_vec());
    }
    let mut original_index = 0;
    let mut expected_original: Option<String> = None;
    while Instant::now() < deadline {
        if !published
            && !fixture
                .state
                .lock()
                .map_err(|_| "lock")?
                .status
                .online
                .is_empty()
        {
            fixture.publish("电脑文字\nHello 👋")?;
            published = true;
        }
        let incoming = fixture.incoming.lock().map_err(|_| "lock")?.take();
        if let Some(clip) = incoming {
            if !text_received {
                assert_eq!(clip.text, "手机文字\nworld 👋");
                text_received = true;
                fixture.publish_image()?;
            } else {
                if let Some(expected) = expected_original.take() {
                    assert_eq!(
                        clip.image.as_deref(),
                        Some(expected.as_str()),
                        "Original image bytes changed in Android clipboard roundtrip"
                    );
                } else {
                    let png = crate::clipboard_image::decode(
                        clip.image.as_deref().ok_or("phone sent no image")?,
                    )
                    .ok_or("invalid phone PNG")?;
                    let dib = crate::clipboard_image::to_dib(&png).ok_or("PNG decode")?;
                    assert_eq!(&dib[124..], &[0, 0, 255, 255, 255, 0, 0, 255]);
                }
                if original_index < originals.len() {
                    expected_original =
                        Some(fixture.publish_image_bytes(&originals[original_index])?);
                    original_index += 1;
                } else {
                    let _ = std::fs::remove_file(path);
                    println!("Android ↔ desktop text, PNG, JPEG and WebP originals verified");
                    return Ok(());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let _ = std::fs::remove_file(path);
    Err("Android test did not finish before timeout".into())
}

#[test]
fn wrong_code_and_expired_offer_never_link() -> Result<()> {
    for expired in [false, true] {
        let fixture = Fixture::new()?;
        if expired {
            fixture.state.lock().map_err(|_| "lock")?.pair_until =
                Some(Instant::now() - Duration::from_secs(1));
        }
        let mut client = fixture.client()?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        write(
            &mut client,
            json!({"type":"hello","version":2,"id":id,"name":"Wrong phone","mode":"join"}),
        )?;
        if !expired {
            let pair = read(&mut client)?;
            let (exchange, message) = crate::pairing::begin(
                "999999",
                &id,
                pair["id"].as_str().ok_or("id")?,
                &crate::hash(&fixture.cert),
            )?;
            let proof = exchange.finish(pair["message"].as_str().ok_or("message")?)?;
            write(
                &mut client,
                json!({"type":"proof","message":message,"proof":proof.tag("client")?}),
            )?;
            assert_eq!(read(&mut client)?["type"], "error");
        } else {
            assert!(read(&mut client).is_err());
        }
        assert!(fixture
            .state
            .lock()
            .map_err(|_| "lock")?
            .config
            .peers
            .is_empty());
    }
    Ok(())
}

#[test]
fn bound_desktops_reconnect_without_discovery_and_reverse_roles_without_pairing() -> Result<()> {
    let a = Fixture::new()?;
    let b = Fixture::new()?;
    a.state.lock().map_err(|_| "lock")?.status.mode = "join".into();
    let aid = a.state.lock().map_err(|_| "lock")?.config.id.clone();
    let bid = b.state.lock().map_err(|_| "lock")?.config.id.clone();
    let link = |client: &Fixture, address: std::net::SocketAddr, remote: String| {
        let state = Arc::clone(&client.state);
        let incoming = Arc::clone(&client.incoming);
        let folder = client.root.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let quit = Arc::clone(&stop);
        (
            stop,
            std::thread::spawn(move || {
                let blocked = socket2::Socket::new(
                    socket2::Domain::IPV4,
                    socket2::Type::STREAM,
                    Some(socket2::Protocol::TCP),
                )
                .map_err(|e| e.to_string())?;
                blocked
                    .bind(
                        &"127.0.0.1:0"
                            .parse::<std::net::SocketAddr>()
                            .map_err(|e| e.to_string())?
                            .into(),
                    )
                    .map_err(|e| e.to_string())?;
                let unavailable = blocked
                    .local_addr()
                    .map_err(|e| e.to_string())?
                    .as_socket()
                    .ok_or("address")?
                    .to_string();
                outbound::connect_candidates(
                    &[unavailable, address.to_string()],
                    &remote,
                    &state,
                    &quit,
                    &incoming,
                    &folder,
                )
            }),
        )
    };
    let wait = |fixture: &Fixture, id: &String, online| -> Result<()> {
        let start = Instant::now();
        while fixture
            .state
            .lock()
            .map_err(|_| "lock")?
            .status
            .online
            .contains(id)
            != online
        {
            if start.elapsed() > Duration::from_secs(6) {
                return Err("connection state timed out".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    };
    // Explicit address pairing learns the remote ID from the PAKE-bound response.
    let (quit, worker) = link(&a, b.address, String::new());
    wait(&a, &bid, true)?;
    let original = token(&aid);
    assert!(!original.is_empty());
    assert!(a.state.lock().map_err(|_| "lock")?.config.peers[0]
        .addresses
        .contains(&b.address.to_string()));
    quit.store(true, Ordering::Relaxed);
    worker.join().map_err(|_| "thread")??;
    wait(&b, &aid, false)?;
    let impostor = Fixture::new()?;
    assert!(outbound::connect(
        impostor.address,
        &bid,
        &a.state,
        &a.stop,
        &a.incoming,
        &a.root
    )
    .is_err());
    assert_eq!(a.state.lock().map_err(|_| "lock")?.config.peers[0].id, bid);
    assert_eq!(original, token(&aid));
    // Now the old TCP server dials its old client. No code or discovery is used.
    let (quit, worker) = link(&b, a.address, aid.clone());
    wait(&b, &aid, true)?;
    assert_eq!(original, token(&aid));
    assert_eq!(a.state.lock().map_err(|_| "lock")?.config.peers.len(), 1);
    assert_eq!(b.state.lock().map_err(|_| "lock")?.config.peers.len(), 1);
    b.publish("same binding, new route")?;
    assert_eq!(
        a.receive(Duration::from_secs(3))?.text,
        "same binding, new route"
    );
    a.publish("reverse direction reply")?;
    assert_eq!(
        b.receive(Duration::from_secs(3))?.text,
        "reverse direction reply"
    );
    quit.store(true, Ordering::Relaxed);
    worker.join().map_err(|_| "thread")??;
    Ok(())
}

#[test]
fn resume_proofs_bind_both_devices_certificate_and_fresh_nonces() -> Result<()> {
    let token = "a".repeat(64);
    let client = "b".repeat(32);
    let server = "c".repeat(32);
    let cert = "d".repeat(64);
    let nonce = "e".repeat(32);
    let challenge = "f".repeat(32);
    let proof = crate::pairing::Proof::resume(&token, &client, &server, &cert, &nonce, &challenge)?;
    let tag = proof.tag("server")?;
    assert!(proof.verify("client", &tag).is_err());
    for changed in [
        crate::pairing::Proof::resume(&token, &server, &client, &cert, &nonce, &challenge)?,
        crate::pairing::Proof::resume(
            &token,
            &client,
            &server,
            &"0".repeat(64),
            &nonce,
            &challenge,
        )?,
        crate::pairing::Proof::resume(&token, &client, &server, &cert, &challenge, &nonce)?,
    ] {
        assert!(changed.verify("server", &tag).is_err());
    }
    Ok(())
}

#[test]
fn old_binding_metadata_keeps_identity_port_and_peers_when_addresses_are_added() -> Result<()> {
    let fixture = Fixture::new()?;
    let local = "a".repeat(32);
    let remote = "b".repeat(32);
    config::write_json(
        &fixture.root.join("config.json"),
        &json!({"enabled":true,"port":54821,"id":local,"name":"Old desktop","peers":[{"id":remote,"name":"Old peer"}]}),
    )?;
    let mut config = load_at(&fixture.root)?;
    assert!(config.peers[0].addresses.is_empty());
    assert!(remember_addresses(
        &mut config,
        &remote,
        ["home:54821".into(), "100.64.0.1:54821".into()]
    ));
    config::write_json(&fixture.root.join("config.json"), &config)?;
    let saved = load_at(&fixture.root)?;
    assert_eq!(saved.id, local);
    assert_eq!(saved.port, 54821);
    assert_eq!(saved.peers[0].id, remote);
    assert_eq!(saved.peers.len(), 1);
    assert_eq!(saved.peers[0].addresses.len(), 2);
    Ok(())
}
