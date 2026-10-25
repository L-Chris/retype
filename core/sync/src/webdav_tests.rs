//! Isolated in-memory WebDAV server; never reads user settings or credentials.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};
struct Mock {
    url: String,
    files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    cstcloud: bool,
    requests: Arc<Mutex<Vec<String>>>,
}
impl Mock {
    fn new() -> Self {
        Self::with_cstcloud(false)
    }
    fn with_cstcloud(cstcloud: bool) -> Self {
        let listener = TcpListener::bind(if cstcloud { "[::1]:0" } else { "127.0.0.1:0" }).unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/dav/", listener.local_addr().unwrap());
        let files = Arc::new(Mutex::new(BTreeMap::<String, Vec<u8>>::new()));
        let copy = Arc::clone(&files);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let Ok((mut socket, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                };
                let _ = socket.set_read_timeout(Some(Duration::from_secs(3)));
                let mut input = Vec::new();
                let mut split = 0;
                let mut length = 0;
                loop {
                    let mut buf = [0u8; 4096];
                    let size = socket.read(&mut buf).unwrap_or(0);
                    if size == 0 {
                        break;
                    }
                    input.extend_from_slice(&buf[..size]);
                    if let Some(offset) = input.windows(4).position(|p| p == b"\r\n\r\n") {
                        split = offset + 4;
                        let header = String::from_utf8_lossy(&input[..offset]).to_lowercase();
                        length = header
                            .lines()
                            .find_map(|l| {
                                l.strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if input.len() >= split + length {
                            break;
                        }
                    }
                }
                // A closed/speculatively opened socket is not an HTTP request.
                // Do not turn an empty connection into a bogus 403/405 response.
                if split == 0 {
                    continue;
                }
                let header = String::from_utf8_lossy(&input[..split]);
                let mut parts = header.lines().next().unwrap_or_default().split_whitespace();
                let method = parts.next().unwrap_or_default();
                let path = parts.next().unwrap_or_default().to_owned();
                captured.lock().unwrap().push(header.to_string());
                let mut files = copy.lock().unwrap();
                let lower = header.to_ascii_lowercase();
                let (code, body) = match method {
                    _ if cstcloud
                        && !lower.contains(&format!(
                            "user-agent: retype/{} zotero/7.0",
                            env!("CARGO_PKG_VERSION")
                        )) =>
                    {
                        (403, vec![])
                    }
                    "PUT"
                        if cstcloud
                            && (!path.ends_with(".prop") || lower.contains("if-none-match:")) =>
                    {
                        (400, vec![])
                    }
                    "MKCOL" => (201, vec![]),
                    "PUT"
                        if header.to_lowercase().contains("if-none-match: *")
                            && files.contains_key(&path) =>
                    {
                        (412, vec![])
                    }
                    "PUT" => {
                        files.insert(
                            path,
                            input
                                .get(split..split + length)
                                .unwrap_or_default()
                                .to_vec(),
                        );
                        (201, vec![])
                    }
                    "GET" => files
                        .get(&path)
                        .map_or((404, vec![]), |bytes| (200, bytes.clone())),
                    "DELETE" => {
                        files.remove(&path);
                        (204, vec![])
                    }
                    "PROPFIND" => {
                        let hrefs = files
                            .keys()
                            .filter(|k| k.starts_with("/dav/retype/v1/devices/"))
                            .map(|k| format!("<d:response><d:href>{k}</d:href></d:response>"))
                            .collect::<String>();
                        (
                            207,
                            format!("<d:multistatus xmlns:d=\"DAV:\">{hrefs}</d:multistatus>")
                                .into_bytes(),
                        )
                    }
                    _ => (405, vec![]),
                };
                drop(files);
                let header = format!(
                    "HTTP/1.1 {code} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(header.as_bytes());
                let _ = socket.write_all(&body);
            }
        });
        Self {
            url,
            files,
            stop,
            worker: Some(worker),
            cstcloud,
            requests,
        }
    }
    fn cloud(&self) -> WebDav {
        WebDav::new(
            &Config {
                url: self.url.clone(),
                username: "fixture".into(),
                provider: if self.cstcloud {
                    "cstcloud"
                } else {
                    "自定义"
                }
                .into(),
                ..Default::default()
            },
            "not-a-user-password",
        )
        .unwrap()
    }
}

#[test]
fn cstcloud_maps_all_operations_and_uses_checked_unconditional_creates() {
    let mock = Mock::with_cstcloud(true);
    let cloud = mock.cloud();
    cloud.test().unwrap();
    // The connection probe is deleted using the same physical file mapping.
    assert!(mock.files.lock().unwrap().is_empty());
    let device = "b".repeat(32);
    for revision in 1..=5 {
        cloud
            .publish(
                &device,
                &serde_json::json!({"device":device,"revision":revision}),
            )
            .unwrap();
    }
    assert_eq!(cloud.devices().unwrap(), vec![device.clone()]);
    let loaded: serde_json::Value = cloud.download(&device).unwrap();
    assert_eq!(loaded["revision"], 5);
    let files = mock.files.lock().unwrap();
    assert!(files.keys().all(|path| path.ends_with(".json.prop")));
    // Current object, three previous objects, and the device index remain.
    assert_eq!(files.len(), 5);
    drop(files);

    let path = "objects/fixture.json";
    cloud.put(path, b"original", true).unwrap();
    let before = mock.requests.lock().unwrap().len();
    cloud.put(path, b"original", true).unwrap();
    assert!(cloud.put(path, b"different", true).is_err());
    let requests = mock.requests.lock().unwrap();
    assert!(requests[before..].iter().all(|r| r.starts_with("GET ")));
    assert!(requests
        .iter()
        .all(|r| !r.to_ascii_lowercase().contains("if-none-match:")));
    let put = requests
        .iter()
        .position(|r| r.starts_with("PUT /dav/retype/v1/objects/fixture.json.prop "))
        .unwrap();
    assert!(requests[put - 1].starts_with("GET /dav/retype/v1/objects/fixture.json.prop "));
    assert!(requests[put + 1].starts_with("GET /dav/retype/v1/objects/fixture.json.prop "));
    drop(requests);
    assert_eq!(cloud.get(path).unwrap().unwrap(), b"original");
}

#[test]
fn custom_cstcloud_host_also_enables_compatibility() {
    let cloud = WebDav::new(
        &Config {
            provider: "自定义".into(),
            url: "https://data.cstcloud.cn/dav/".into(),
            username: "fixture".into(),
            ..Default::default()
        },
        "not-a-user-password",
    )
    .unwrap();
    assert!(cloud.cstcloud);
    assert!(!Mock::new().cloud().cstcloud);
}
impl Drop for Mock {
    fn drop(&mut self) {
        if std::thread::panicking() {
            for header in self.requests.lock().unwrap().iter() {
                eprintln!(
                    "fixture request: {} ; {}",
                    header.lines().next().unwrap_or("empty"),
                    header
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("user-agent:"))
                        .unwrap_or("no user-agent")
                );
            }
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[test]
fn published_versions_are_checked_and_interrupted_uploads_keep_the_last_index() {
    let mock = Mock::new();
    let cloud = mock.cloud();
    cloud.test().unwrap();
    let device = "a".repeat(32);
    let make = |revision| serde_json::json!({"version":1,"device":device,"revision":revision});
    cloud.publish(&device, &make(1)).unwrap();
    assert_eq!(cloud.devices().unwrap(), vec![device.clone()]);
    let index_path = format!("/dav/retype/v1/devices/{device}.json");
    let index = mock.files.lock().unwrap()[&index_path].clone();
    // An abandoned object is not a committed snapshot.
    cloud.put("objects/abandoned.json", br#"{}"#, true).unwrap();
    assert_eq!(mock.files.lock().unwrap()[&index_path], index);
    for revision in 2..=7 {
        cloud.publish(&device, &make(revision)).unwrap();
    }
    let index: Index = serde_json::from_slice(&mock.files.lock().unwrap()[&index_path]).unwrap();
    assert_eq!(index.previous.len(), 3);
    let loaded: serde_json::Value = cloud.download(&device).unwrap();
    assert_eq!(loaded["revision"], 7);
    let object = format!("/dav/retype/v1/objects/{}.json", index.hash);
    mock.files
        .lock()
        .unwrap()
        .insert(object, br#"{"wrong":1}"#.to_vec());
    assert!(cloud.download::<serde_json::Value>(&device).is_err());
}
