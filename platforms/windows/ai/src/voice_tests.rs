#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use std::{io::Write, net::TcpListener, thread};
fn fixture(bodies: Vec<String>) -> (Provider, thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        let started = std::time::Instant::now();
        let mut quiet_since = None::<std::time::Instant>;
        loop {
            let (mut socket, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if quiet_since.is_some_and(|t| t.elapsed() >= Duration::from_millis(300)) {
                        break;
                    }
                    assert!(
                        started.elapsed() < Duration::from_secs(10),
                        "missing voice request"
                    );
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(e) => panic!("voice fixture accept failed: {e}"),
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let mut length = 0;
            let mut line = String::new();
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(size) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = size.trim().parse::<usize>().unwrap();
                }
            }
            let mut data = vec![0; length];
            reader.read_exact(&mut data).unwrap();
            let body = bodies
                .get(requests.len())
                .cloned()
                .unwrap_or_else(|| output("unexpected duplicate", "stop"));
            requests.push(serde_json::from_slice(&data).unwrap());
            let header=format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
            socket.write_all(header.as_bytes()).unwrap();
            // Deliberately split transport writes across UTF-8 and JSON boundaries.
            for bytes in body.as_bytes().chunks(7) {
                let _ = socket.write_all(bytes);
            }
            if requests.len() >= bodies.len() {
                quiet_since = Some(std::time::Instant::now());
            }
        }
        requests
    });
    (
        Provider {
            base_url: format!("http://{address}/v1"),
            models: vec!["audio-model".into()],
            ..Default::default()
        },
        handle,
    )
}
fn output(text: &str, reason: &str) -> String {
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"delta":{"content":text,"reasoning_content":"must not be shown"}}]}),
        json!({"choices":[{"delta":{},"finish_reason":reason}]})
    )
}
fn settings() -> Settings {
    Settings {
        model: "audio-model".into(),
        ..Default::default()
    }
}
fn wait(session: &Session) -> Snapshot {
    let start = std::time::Instant::now();
    loop {
        let state = session.snapshot();
        if !matches!(state.phase, Phase::Recording | Phase::Recognizing) {
            return state;
        }
        assert!(start.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn audio_data_uri_streams_content_and_excludes_reasoning() {
    let (p, server) = fixture(vec![output("你好，world!", "stop")]);
    let mut previews = Vec::new();
    let text = transcribe(
        &p,
        &settings(),
        &[1000, -1000],
        "test-key",
        &AtomicBool::new(false),
        |s| previews.push(s.to_owned()),
    )
    .unwrap();
    assert_eq!(text, "你好，world!");
    assert!(!previews.iter().any(|s| s.contains("must not")));
    let requests = server.join().unwrap();
    let body = &requests[0];
    assert_eq!(body["enable_thinking"], false);
    assert_eq!(body["reasoning_effort"], "none");
    let uri = body["messages"][0]["content"][1]["input_audio"]["data"]
        .as_str()
        .unwrap();
    let wav = STANDARD
        .decode(uri.strip_prefix("data:audio/wav;base64,").unwrap())
        .unwrap();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[44..], &[232, 3, 24, 252]);
}
#[test]
fn truncation_and_missing_final_marker_are_rejected() {
    for body in [
        output("partial", "length"),
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\ndata: [DONE]\n\n".into(),
    ] {
        let (p, server) = fixture(vec![body]);
        assert!(transcribe(
            &p,
            &settings(),
            &[1000],
            "test",
            &AtomicBool::new(false),
            |_| {}
        )
        .is_err());
        server.join().unwrap();
    }
}
#[test]
fn short_recording_sends_one_complete_request_even_when_stopped_twice() {
    let (p, server) = fixture(vec![output("最终文字。", "stop")]);
    let s = Session::start(p, settings(), "test".into()).unwrap();
    s.push(vec![1000; 320]).unwrap();
    s.stop().unwrap();
    s.stop().unwrap();
    assert!(s.push(vec![1]).is_err());
    let state = wait(&s);
    assert_eq!(state.phase, Phase::Done);
    assert_eq!(state.text, "最终文字。");
    assert!(state.error.is_none());
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 1);
    for r in requests {
        let uri = r["messages"][0]["content"][1]["input_audio"]["data"]
            .as_str()
            .unwrap();
        assert_eq!(
            STANDARD
                .decode(uri.strip_prefix("data:audio/wav;base64,").unwrap())
                .unwrap()
                .len(),
            684
        );
    }
}
#[test]
fn long_file_recording_has_no_preview_and_uploads_all_speech_once() {
    let (p, server) = fixture(vec![output("跨段定稿与尾句。", "stop")]);
    let s = Session::start(p, settings(), "test".into()).unwrap();
    for _ in 0..2 {
        s.push(vec![1000; voice::RATE]).unwrap();
        s.push(vec![0; voice::RATE * 7 / 10]).unwrap();
    }
    s.push(vec![2000; 320]).unwrap();
    thread::sleep(Duration::from_millis(150));
    assert_eq!(s.snapshot().phase, Phase::Recording);
    assert!(s.snapshot().text.is_empty());
    s.stop().unwrap();
    let state = wait(&s);
    assert_eq!(state.phase, Phase::Done);
    assert_eq!(state.text, "跨段定稿与尾句。");
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 1);
    let uri = requests[0]["messages"][0]["content"][1]["input_audio"]["data"]
        .as_str()
        .unwrap();
    let data = STANDARD
        .decode(uri.strip_prefix("data:audio/wav;base64,").unwrap())
        .unwrap();
    assert_eq!(
        data.len(),
        44 + (2 * (voice::RATE + voice::RATE * 7 / 10) + 320) * 2
    );
    assert_eq!(&data[data.len() - 2..], &2000i16.to_le_bytes());
}
#[test]
fn truncated_file_result_never_exposes_partial_text_or_retries() {
    let (p, server) = fixture(vec![output("截断尾段", "length")]);
    let s = Session::start(p, settings(), "test".into()).unwrap();
    s.push(vec![1000; voice::RATE]).unwrap();
    s.push(vec![0; voice::RATE * 7 / 10]).unwrap();
    s.stop().unwrap();
    let state = wait(&s);
    assert_eq!(state.phase, Phase::Error);
    assert!(state.text.is_empty());
    assert_eq!(server.join().unwrap().len(), 1);
}
#[test]
fn cancellation_wins_over_late_updates_and_silence_never_calls_a_model() {
    let s = Session::start(Provider::default(), settings(), String::new()).unwrap();
    s.cancel();
    update(&s.snapshot, &s.cancelled, |state| {
        state.text = "late".into()
    });
    assert_eq!(s.snapshot().phase, Phase::Cancelled);
    assert!(s.snapshot().text.is_empty());
    let s = Session::start(Provider::default(), settings(), String::new()).unwrap();
    s.push(vec![0; 320]).unwrap();
    s.stop().unwrap();
    assert_eq!(wait(&s).error.as_deref(), Some("没有检测到语音"));
}
#[test]
fn gemini_file_audio_uses_native_inline_data_and_excludes_thoughts() {
    let body = json!({"candidates":[{"finishReason":"STOP","content":{"parts":[
        {"thought":true,"text":"hidden reasoning"},{"text":"原文。"}
    ]}}]})
    .to_string();
    let (mut provider, server) = fixture(vec![body]);
    provider.kind = ApiKind::Gemini;
    let session = Session::start(provider, settings(), "test".into()).unwrap();
    session.push(vec![1000; 320]).unwrap();
    session.stop().unwrap();
    let state = wait(&session);
    assert_eq!(state.phase, Phase::Done);
    assert_eq!(state.text, "原文。");
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 1);
    let audio = &requests[0]["contents"][0]["parts"][1]["inlineData"];
    assert_eq!(audio["mimeType"], "audio/wav");
    let data = STANDARD.decode(audio["data"].as_str().unwrap()).unwrap();
    assert_eq!(&data[..4], b"RIFF");
}
