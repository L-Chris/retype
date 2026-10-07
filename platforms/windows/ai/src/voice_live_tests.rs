#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use crate::{config::VoiceProtocol, voice_service::Session};
use std::{net::TcpListener, thread};

fn provider(url: String) -> Provider {
    Provider {
        base_url: url,
        models: vec!["audio-model".into()],
        voice_protocols: [("audio-model".into(), VoiceProtocol::GeminiLive)].into(),
        ..Default::default()
    }
}
fn settings() -> Settings {
    Settings {
        model: "audio-model".into(),
        tidy: true,
        language: "zh".into(),
        ..Default::default()
    }
}
fn wait_for(s: &Session, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let start = Instant::now();
    loop {
        let snapshot = s.snapshot();
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "voice state did not settle: {snapshot:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn live_audio_uses_one_socket_replaces_interim_and_waits_for_late_final() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = provider(format!("http://{}", listener.local_addr().unwrap()));
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let setup: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(setup["setup"]["model"], "models/audio-model");
        assert_eq!(setup["setup"]["inputAudioTranscription"]["mode"], "SMART");
        assert_eq!(
            setup["setup"]["inputAudioTranscription"]["languageCodes"],
            json!(["cmn-Hans-CN"])
        );
        assert_eq!(
            setup["setup"]["realtimeInputConfig"]["automaticActivityDetection"]["disabled"],
            true
        );
        ws.send(Message::Binary(
            json!({"setupComplete":{}}).to_string().into_bytes().into(),
        ))
        .unwrap();
        let mut pcm = Vec::new();
        let mut started = false;
        loop {
            let msg = ws.read().unwrap();
            if !msg.is_text() {
                continue;
            }
            let v: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            if v["realtimeInput"].get("activityStart").is_some() {
                started = true;
            }
            if let Some(data) = v["realtimeInput"]["audio"]["data"].as_str() {
                assert!(started);
                pcm.extend(STANDARD.decode(data).unwrap());
                for text in ["临时", "修正后的预览"] {
                    ws.send(Message::Text(
                        json!({"serverContent":{"interimInputTranscription":{"text":text}}})
                            .to_string()
                            .into(),
                    ))
                    .unwrap();
                }
            }
            if v["realtimeInput"].get("activityEnd").is_some() {
                // Google documents independent ordering of transcription and turn completion.
                ws.send(Message::Text(
                    json!({"serverContent":{"generationComplete":true}})
                        .to_string()
                        .into(),
                ))
                .unwrap();
                thread::sleep(Duration::from_millis(350));
                ws.send(Message::Text(
                    json!({"serverContent":{"inputTranscription":{"text":"最终文字。"}}})
                        .to_string()
                        .into(),
                ))
                .unwrap();
                let _ = ws.read();
                return pcm;
            }
        }
    });
    let session = Session::start(p, settings(), "test".into()).unwrap();
    session.push(vec![1000, -1000]).unwrap();
    let preview = wait_for(&session, |s| s.text == "修正后的预览");
    assert_eq!(preview.phase, Phase::Recording);
    session.stop().unwrap();
    thread::sleep(Duration::from_millis(150));
    assert_ne!(session.snapshot().phase, Phase::Done);
    let result = wait_for(&session, |s| {
        !matches!(s.phase, Phase::Recording | Phase::Recognizing)
    });
    assert_eq!(result.phase, Phase::Done);
    assert_eq!(result.text, "最终文字。");
    assert_eq!(server.join().unwrap(), vec![232, 3, 24, 252]);
}
#[test]
fn preview_cannot_become_final_when_socket_closes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = provider(format!("http://{}", listener.local_addr().unwrap()));
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let _ = ws.read();
        ws.send(Message::Text(
            json!({"setupComplete":{}}).to_string().into(),
        ))
        .unwrap();
        loop {
            let msg = ws.read().unwrap();
            if msg.to_text().unwrap_or("").contains("activityEnd") {
                ws.send(Message::Text(json!({"serverContent":{"interimInputTranscription":{"text":"不完整"},"turnComplete":true}}).to_string().into())).unwrap();
                let _ = ws.close(None);
                return;
            }
        }
    });
    let session = Session::start(p, settings(), "test".into()).unwrap();
    session.push(vec![1000; 320]).unwrap();
    session.stop().unwrap();
    assert_eq!(
        wait_for(&session, |s| s.phase == Phase::Error).phase,
        Phase::Error
    );
    server.join().unwrap();
}
#[test]
fn completion_and_final_transcription_can_arrive_in_either_order() {
    for marker in ["turnComplete", "generationComplete"] {
        for completion_first in [false, true] {
            let mut state = Transcript::default();
            let complete = json!({"serverContent":{marker:true}});
            let final_text = json!({"serverContent":{"inputTranscription":{"text":"完成"}}});
            let (first, last) = if completion_first {
                (&complete, &final_text)
            } else {
                (&final_text, &complete)
            };
            state.accept(first, true).unwrap();
            assert!(!(state.turn_complete && state.final_after_end));
            state.accept(last, true).unwrap();
            assert!(state.turn_complete && state.final_after_end);
            assert_eq!(state.stable, "完成");
        }
    }
}
#[test]
fn live_connection_test_only_sets_up_and_never_records_or_translates() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = provider(format!("http://{}", listener.local_addr().unwrap()));
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let first = ws.read().unwrap();
        assert!(first.to_text().unwrap().contains("setup"));
        ws.send(Message::Text(
            json!({"setupComplete":{}}).to_string().into(),
        ))
        .unwrap();
        assert!(ws.read().unwrap().is_close());
    });
    crate::service::test_with_key(&p, "audio-model", "test").unwrap();
    server.join().unwrap();
}
#[test]
fn capability_defaults_and_overrides_survive_old_configs() {
    let mut p: Provider = serde_json::from_value(json!({"id":"g","preset":"Gemini","name":"Gemini","kind":"Gemini","base_url":"https://generativelanguage.googleapis.com/v1beta","models":["gemini-3.5-transcribe-live"]})).unwrap();
    assert_eq!(
        p.voice_protocol("gemini-3.5-transcribe-live"),
        VoiceProtocol::GeminiLive
    );
    assert_eq!(
        p.voice_protocol("some-live-looking-text-model"),
        VoiceProtocol::File
    );
    p.voice_protocols
        .insert("alias".into(), VoiceProtocol::GeminiLive);
    let restored: Provider = serde_json::from_value(serde_json::to_value(p).unwrap()).unwrap();
    assert_eq!(restored.voice_protocol("alias"), VoiceProtocol::GeminiLive);
}
#[test]
fn transport_errors_do_not_expose_credentials_or_server_text() {
    let response = tungstenite::http::Response::builder()
        .status(403)
        .body(Some(b"secret-key".to_vec()))
        .unwrap();
    let error = socket_error(tungstenite::Error::Http(Box::new(response)));
    assert!(!error.contains("secret-key"));
    assert!(error.contains("鉴权"));
    for base in [
        "https://user:secret@host/v1beta",
        "https://host?key=secret",
        "http://example.com",
    ] {
        assert!(http_base(&provider(base.into())).is_err());
    }
}
