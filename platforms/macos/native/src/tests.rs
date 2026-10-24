#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use std::time::Instant;

fn db_path() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "retype-macos-{}-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
fn runtime(path: &Path) -> Arc<Runtime> {
    let (dict, _) = retype_dict::from_pairs([
        ("你好", "ni hao", 10000.),
        ("我是", "wo shi", 10000.),
        ("我", "wo", 1000.),
        ("额度", "e du", 10000.),
        ("你的", "ni de", 5000.),
        ("你", "ni", 1000.),
    ]);
    Runtime::with_dictionary(Arc::new(dict), path).unwrap()
}
fn key(session: &mut Session, text: &str) -> Value {
    session
        .dispatch(Command::Key {
            value: text.into(),
            mods: 0,
        })
        .unwrap()
}
fn type_text(session: &mut Session, text: &str) -> Value {
    let mut result = Value::Null;
    for c in text.chars() {
        result = key(session, &c.to_string());
    }
    result
}
#[test]
fn schemes_incomplete_syllables_and_backspace_use_shared_engine() {
    let runtime = runtime(&db_path());
    for (flypy, input, expected) in [
        (false, "nihao", "你好"),
        (true, "nihc", "你好"),
        (true, "wou", "我是"),
        (true, "edu", "额度"),
    ] {
        let mut session = Session::new(Arc::clone(&runtime), flypy, true);
        let result = type_text(&mut session, input);
        assert!(
            result["candidates"]
                .as_array()
                .unwrap()
                .contains(&json!(expected)),
            "{input}: {result}"
        );
        assert_eq!(
            key(&mut session, "backspace")["composition"],
            &input[..input.len() - 1]
        );
        session.dispatch(Command::Reset).unwrap();
        assert_eq!(session.dispatch(Command::Poll).unwrap()["composition"], "");
    }
}
#[test]
fn deletion_to_two_letters_and_late_worker_results_stay_consistent() {
    let mut session = Session::new(runtime(&db_path()), true, true);
    let started = Instant::now();
    type_text(&mut session, "wdsssssssssss");
    for _ in 0..11 {
        key(&mut session, "backspace");
    }
    assert_eq!(
        session.dispatch(Command::Poll).unwrap()["composition"],
        "wd"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    session.dispatch(Command::Reset).unwrap();
    assert_eq!(
        session.dispatch(Command::Poll).unwrap()["candidates"],
        json!([])
    );
}
#[test]
fn measured_pages_fit_width_and_keep_number_selection_in_sync() {
    let words: Vec<_> = (0..40).map(|i| format!("候选{i}")).collect();
    let (dict, _) = retype_dict::from_pairs(
        words
            .iter()
            .enumerate()
            .map(|(i, w)| (w.as_str(), "ni", (100 - i) as f64)),
    );
    let runtime = Runtime::with_dictionary(Arc::new(dict), &db_path()).unwrap();
    let mut session = Session::new(runtime, false, true);
    let result = type_text(&mut session, "ni");
    let generation = result["generation"].as_u64().unwrap();
    let count = result["candidates"].as_array().unwrap().len();
    let layout = session
        .dispatch(Command::Layout {
            generation,
            widths: vec![90; count],
            available: 472,
        })
        .unwrap();
    assert_eq!(layout["pageSize"], 5);
    let next = key(&mut session, "=");
    assert_eq!(next["pageStart"], 5);
    let expected = next["candidates"][5].clone();
    assert_eq!(key(&mut session, "1")["commits"][0], expected);
    type_text(&mut session, "ni");
    let state = session.backend.render();
    let narrow = session
        .dispatch(Command::Layout {
            generation: state.gen,
            widths: vec![30; state.candidates.len()],
            available: 472,
        })
        .unwrap();
    assert_eq!(narrow["pageSize"], 8);
}
#[test]
fn stale_choices_layouts_and_unrecognized_shortcuts_do_not_commit() {
    let mut session = Session::new(runtime(&db_path()), false, true);
    let old = type_text(&mut session, "ni");
    key(&mut session, "h");
    let generation = old["generation"].as_u64().unwrap();
    assert_eq!(
        session
            .dispatch(Command::Choose {
                index: 0,
                generation
            })
            .unwrap()["commits"],
        json!([])
    );
    let before = session.backend.render();
    session
        .dispatch(Command::Layout {
            generation,
            widths: vec![400; before.candidates.len()],
            available: 472,
        })
        .unwrap();
    assert_eq!(session.backend.render().page_starts, before.page_starts);
    let result = session
        .dispatch(Command::Key {
            value: "a".into(),
            mods: Modifiers::WIN.bits(),
        })
        .unwrap();
    assert_eq!(result["passThrough"], true);
    assert_eq!(result["commits"], json!([]));
    assert_eq!(result["composition"], "");
}
#[test]
fn english_completion_numbers_and_original_space_have_distinct_behavior() {
    let mut session = Session::new(runtime(&db_path()), false, false);
    let result = type_text(&mut session, "hel");
    assert!(result["candidates"]
        .as_array()
        .unwrap()
        .contains(&json!("hello")));
    let index = result["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .position(|v| v == "hello")
        .unwrap();
    let chosen = session
        .dispatch(Command::Choose {
            index,
            generation: result["generation"].as_u64().unwrap(),
        })
        .unwrap();
    assert_eq!(chosen["commits"], json!(["hello "]));
    type_text(&mut session, "hel");
    assert_eq!(key(&mut session, "space")["commits"], json!(["hel "]));
    let result = type_text(&mut session, "hel");
    let expected = format!("{} ", result["candidates"][0].as_str().unwrap());
    assert_eq!(key(&mut session, "1")["commits"], json!([expected]));
}
#[test]
fn accepted_learning_is_shared_and_persisted_but_rejected_or_stale_ack_is_not() {
    let path = db_path();
    let runtime = runtime(&path);
    let mut session = Session::new(Arc::clone(&runtime), false, false);
    for accepted in [false, true, true] {
        type_text(&mut session, "retypecustomword");
        let result = key(&mut session, "space");
        let token = result["token"].as_u64().unwrap();
        session
            .dispatch(Command::Ack {
                token: token + 1,
                accepted: true,
            })
            .unwrap();
        session.dispatch(Command::Ack { token, accepted }).unwrap();
        session
            .dispatch(Command::Ack {
                token,
                accepted: true,
            })
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime
        .learner
        .english_snapshot()
        .iter()
        .find(|(w, _)| w == "retypecustomword")
        .map(|(_, n)| *n)
        != Some(2)
    {
        assert!(Instant::now() < deadline, "learning worker did not flush");
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut other = Session::new(Arc::clone(&runtime), false, false);
    assert!(type_text(&mut other, "retypecus")["candidates"]
        .as_array()
        .unwrap()
        .contains(&json!("retypecustomword")));
    let reopened = Runtime::with_dictionary(Arc::clone(&runtime.dictionary), &path).unwrap();
    assert_eq!(
        reopened
            .learner
            .english_snapshot()
            .iter()
            .find(|(w, _)| w == "retypecustomword")
            .map(|(_, n)| *n),
        Some(2)
    );
}
#[test]
fn c_boundary_rejects_bad_commands_and_missing_resources_without_unwinding() {
    unsafe {
        let runtime = RtRuntime(runtime(&db_path()));
        let session = rt_session_new(&runtime, true, true);
        assert!(!session.is_null());
        for command in [
            "not json",
            r#"{"type":"key","value":"two letters"}"#,
            r#"{"type":"unknown"}"#,
        ] {
            let command = CString::new(command).unwrap();
            let response = rt_session_send(session, command.as_ptr());
            let value: Value =
                serde_json::from_str(CStr::from_ptr(response).to_str().unwrap()).unwrap();
            rt_string_free(response);
            assert!(value["error"].is_string());
        }
        let root = CString::new(db_path().to_str().unwrap()).unwrap();
        let mut missing = std::ptr::null_mut();
        let response = rt_runtime_open(root.as_ptr(), &mut missing);
        assert!(missing.is_null());
        rt_string_free(response);
        let resources = rt_resources_new(root.as_ptr());
        rt_resources_begin(resources);
        rt_resources_cancel(resources);
        let response = rt_resources_run(resources, 1, std::ptr::null());
        let value: Value =
            serde_json::from_str(CStr::from_ptr(response).to_str().unwrap()).unwrap();
        assert!(value["error"].as_str().unwrap().contains("取消"));
        rt_string_free(response);
        rt_resources_free(resources);
        rt_session_free(session);
    }
}

#[test]
fn termination_barrier_flushes_only_acknowledged_learning_and_numeric_statistics() {
    let path = db_path();
    let runtime = runtime(&path);
    let mut session = Session::new(Arc::clone(&runtime), false, false);
    for _ in 0..2 {
        type_text(&mut session, "macospersistword");
        let result = key(&mut session, "space");
        session
            .dispatch(Command::Ack {
                token: result["token"].as_u64().unwrap(),
                accepted: true,
            })
            .unwrap();
    }
    let handle = RtRuntime(Arc::clone(&runtime));
    unsafe {
        let output = rt_runtime_flush(&handle);
        let result: Value = serde_json::from_str(CStr::from_ptr(output).to_str().unwrap()).unwrap();
        rt_string_free(output);
        assert_eq!(result, true);
    }
    let rows =
        retype_sync::statistics::capture(&path.with_extension("statistics"), &"a".repeat(32))
            .unwrap();
    assert_eq!(rows.iter().map(|r| r.counts.english_words).sum::<u64>(), 2);
    let logs = std::fs::read_to_string(path.with_extension("statistics").join(
        retype_sync::statistics::versioned_stream("macos.log", env!("CARGO_PKG_VERSION")),
    ))
    .unwrap();
    assert!(!logs.contains("macospersistword"));
    assert_eq!(
        runtime
            .learner
            .english_snapshot()
            .iter()
            .find(|(w, _)| w == "macospersistword")
            .map(|(_, n)| *n),
        Some(2)
    );
}

#[test]
fn input_measurements_wait_for_ack_ignore_replays_and_cancel_without_success() {
    let path = db_path();
    let runtime = runtime(&path);
    let mut session = Session::new(Arc::clone(&runtime), false, true);
    let totals = |runtime: &Runtime| {
        runtime.statistics.flush().unwrap();
        let mut total = retype_types::statistics::InputStats::default();
        for row in
            retype_sync::statistics::capture(&path.with_extension("statistics"), &"a".repeat(32))
                .unwrap()
        {
            total.add(&row.counts.input);
        }
        total
    };
    for accepted in [false, true] {
        type_text(&mut session, "nihao");
        key(&mut session, "backspace");
        key(&mut session, "o");
        let result = key(&mut session, "space");
        let token = result["token"].as_u64().unwrap();
        assert_eq!(totals(&runtime).total(true).inputs, 0);
        session.dispatch(Command::Ack { token, accepted }).unwrap();
        session
            .dispatch(Command::Ack {
                token,
                accepted: true,
            })
            .unwrap();
    }
    let input = totals(&runtime);
    assert_eq!(
        (
            input.total(true).inputs,
            input.total(true).corrected,
            input.total(true).units
        ),
        (1, 1, 2)
    );
    type_text(&mut session, "ni");
    key(&mut session, "escape");
    assert_eq!(totals(&runtime).cancelled, 1);
    type_text(&mut session, "ni");
    session.dispatch(Command::Discard).unwrap();
    assert_eq!(totals(&runtime).cancelled, 1);
}
