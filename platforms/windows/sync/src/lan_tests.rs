use super::*;
use std::io::{BufRead, BufReader};
type Client = BufReader<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>;

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
            }
        }
        let _ = std::fs::remove_file(self.root.join("config.json"));
        let _ = std::fs::remove_dir(&self.root);
    }
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
    fixture.publish("电脑\nHello 👋")?;
    let from_pc = read(&mut client)?;
    assert_eq!(from_pc["clip"]["text"], "电脑\nHello 👋");
    let clip = Clip {
        origin: id.clone(),
        seq: 1,
        clock: now() + 100,
        created: now(),
        text: "手机\nworld".into(),
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
        if let Some(clip) = fixture.incoming.lock().map_err(|_| "lock")?.take() {
            assert_eq!(clip.text, "手机文字\nworld 👋");
            let _ = std::fs::remove_file(path);
            println!("Android → Windows and Windows → Android text verified");
            return Ok(());
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
