use super::*;
use std::time::Duration;

pub fn download(
    root: &Path,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<Status> {
    let _lease = Lease::acquire(root)?;
    let catalog = Catalog::builtin()?;
    fs::create_dir_all(root.join("downloads")).map_err(|e| e.to_string())?;
    let total = catalog
        .components
        .iter()
        .map(|c| c.compressed_bytes)
        .sum::<u64>();
    let mut completed = 0;
    for component in &catalog.components {
        if component.present(root) && checksum(&component.path(root))? == component.sha256 {
            completed += component.compressed_bytes;
            progress(completed as f32 / total as f32);
            continue;
        }
        let part = component.partial(root);
        fetch(component, &part, &component.url(), cancelled, |received| {
            progress((completed + received) as f32 / total as f32)
        })?;
        install_component(root, component, &part, cancelled)?;
        completed += component.compressed_bytes;
        progress(completed as f32 / total as f32);
        let _ = fs::remove_file(part);
    }
    status(root)
}
fn fetch(
    component: &Component,
    part: &Path,
    url: &str,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("下载已取消".into());
    }
    let mut start = fs::metadata(part).map_or(0, |m| m.len());
    if start == component.compressed_bytes && checksum(part)? == component.compressed_sha256 {
        return Ok(());
    }
    if start >= component.compressed_bytes {
        start = 0;
    }
    let agent = ureq::Agent::config_builder()
        .https_only(!cfg!(test))
        .max_redirects(6)
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_global(Some(Duration::from_secs(600)))
        .build()
        .into();
    let agent: ureq::Agent = agent;
    let mut request = agent.get(url).header("Accept-Encoding", "identity");
    if start > 0 {
        request = request.header("Range", format!("bytes={start}-"));
    }
    let mut response = request
        .call()
        .map_err(|_| "词库下载失败，请检查网络后重试")?;
    if response.status().as_u16() == 206 {
        let expected = format!(
            "bytes {start}-{}/{}",
            component.compressed_bytes - 1,
            component.compressed_bytes
        );
        if response
            .headers()
            .get("Content-Range")
            .and_then(|v| v.to_str().ok())
            != Some(expected.as_str())
        {
            return Err("词库续传响应无效".into());
        }
    } else if response.status().as_u16() == 200 {
        start = 0;
    } else {
        return Err("词库下载响应无效".into());
    }
    if response
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|size| size != component.compressed_bytes - start)
    {
        return Err("词库下载大小无效".into());
    }
    let mut output = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(start == 0)
        .open(part)
        .map_err(|e| e.to_string())?;
    use std::io::{Seek, SeekFrom};
    output
        .seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    let mut received = start;
    let mut buffer = [0; 65536];
    let mut input = response.body_mut().as_reader();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("下载已取消".into());
        }
        let n = input
            .read(&mut buffer)
            .map_err(|_| "网络连接中断，可重试继续下载")?;
        if n == 0 {
            break;
        }
        received += n as u64;
        if received > component.compressed_bytes {
            return Err("词库下载超出大小限制".into());
        }
        output.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        progress(received);
    }
    output.sync_all().map_err(|e| e.to_string())?;
    drop(output);
    if received != component.compressed_bytes {
        return Err("词库下载未完成，可重试继续下载".into());
    }
    if checksum(part)? != component.compressed_sha256 {
        let _ = fs::remove_file(part);
        return Err("词库下载校验失败，请重试".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn serve(
        body: &'static [u8],
        range: bool,
        invalid: bool,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/resource", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut b = [0];
                socket.read_exact(&mut b).unwrap();
                request.push(b[0]);
            }
            assert!(String::from_utf8_lossy(&request)
                .to_lowercase()
                .contains("range: bytes=3-"));
            let header = if range {
                format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",body.len()-3,if invalid {2}else{3},body.len()-1,body.len())
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
            };
            socket.write_all(header.as_bytes()).unwrap();
            socket
                .write_all(if range { &body[3..] } else { body })
                .unwrap();
        });
        (url, thread)
    }
    #[test]
    fn interrupted_download_resumes_or_restarts_and_rejects_wrong_ranges() {
        let root = crate::tests::Temp::new();
        let body = b"a compressed resource";
        let c = Component {
            id: "english".into(),
            format: "RTENGL01".into(),
            bytes: 30,
            sha256: "0".repeat(64),
            compressed_bytes: body.len() as u64,
            compressed_sha256: hex(&Sha256::digest(body)),
            release: None,
        };
        let part = root.0.join("download.part");
        for (range, invalid) in [(true, false), (false, false), (true, true)] {
            fs::write(&part, &body[..3]).unwrap();
            let (url, server) = serve(body, range, invalid);
            let result = fetch(&c, &part, &url, &AtomicBool::new(false), |_| {});
            server.join().unwrap();
            assert_eq!(result.is_ok(), !invalid);
            assert_eq!(
                fs::read(&part).unwrap(),
                if invalid {
                    body[..3].to_vec()
                } else {
                    body.to_vec()
                }
            );
        }
    }
}
