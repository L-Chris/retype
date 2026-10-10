use crate::{offline_cloud, Kernel, KernelConfig};
use retype_dict::{from_pairs, Learner, UserDict};
use retype_types::*;
use std::{sync::Arc, time::Duration};

fn kernel(scheme: PinyinScheme) -> Kernel {
    let (dict, _) = from_pairs([
        ("拼", "pin", 1.0),
        ("安", "an", 1.0),
        ("你", "ni", 1000.0),
        ("你好", "ni hao", 100.0),
        ("擦", "ca", 100000.0),
    ]);
    Kernel::new(
        KernelConfig {
            pinyin_scheme: scheme,
            rerank_enabled: false,
            ..Default::default()
        },
        Arc::new(dict),
        Arc::new(Learner::new(Arc::new(UserDict::new()))),
        offline_cloud(Duration::from_millis(10)),
    )
}
fn key(k: &mut Kernel, key: Key) -> Vec<KernelAction> {
    k.handle(InputEvent::Key {
        key,
        mods: Modifiers::NONE,
        source: InputSource::Keyboard,
    })
}
fn input(k: &mut Kernel, text: &str) {
    for c in text.chars() {
        key(k, Key::Char(c));
    }
}
fn committed(actions: &[KernelAction]) -> Option<&str> {
    actions.iter().find_map(|a| match a {
        KernelAction::Commit(CommitRequest::ReplaceComposition { text }) => Some(text.as_str()),
        _ => None,
    })
}

#[test]
fn both_schemes_select_english_with_space_numbers_or_click_without_extra_space() {
    for scheme in [PinyinScheme::Full, PinyinScheme::Flypy] {
        for selection in 0..3 {
            let mut k = kernel(scheme);
            input(&mut k, "hello");
            let render = k.render_state();
            assert_eq!(render.candidates[0].text, "hello");
            assert_eq!(render.candidates[0].language, CandidateLanguage::English);
            assert_eq!(render.candidates[0].consumed, 5);
            let actions = match selection {
                0 => key(&mut k, Key::Space),
                1 => key(&mut k, Key::Char('1')),
                _ => k.handle(InputEvent::CandidateChosen { index: 0 }),
            };
            assert_eq!(committed(&actions), Some("hello"));
            assert!(actions
                .iter()
                .any(|a| matches!(a, KernelAction::Side(SideEffect::Learn(
                LearningEvent::EnglishWord{text})) if text == "hello")));
            assert!(!actions.iter().any(|a| matches!(
                a,
                KernelAction::Side(SideEffect::Learn(LearningEvent::CandidateChosen { .. }))
            )));
            assert!(!k.has_composition());
            assert!(k.is_chinese());
        }
    }
}

#[test]
fn chinese_words_win_conflicts_and_english_completions_are_bounded() {
    for (scheme, spelling, chinese) in [
        (PinyinScheme::Full, "pin", "拼"),
        (PinyinScheme::Flypy, "an", "安"),
    ] {
        let mut k = kernel(scheme);
        input(&mut k, spelling);
        let render = k.render_state();
        assert_eq!(render.candidates[0].text, chinese);
        assert_eq!(render.candidates[1].text.to_ascii_lowercase(), spelling);
        assert!(
            render
                .candidates
                .iter()
                .filter(|c| c.language == CandidateLanguage::English)
                .count()
                <= 2
        );
    }
    let mut k = kernel(PinyinScheme::Full);
    input(&mut k, "hel");
    assert!(k
        .render_state()
        .candidates
        .iter()
        .any(|c| c.language == CandidateLanguage::English && c.text.starts_with("hel")));
    input(&mut k, "zzzz");
    assert!(!k
        .render_state()
        .candidates
        .iter()
        .any(|c| c.language == CandidateLanguage::English));
    for _ in 0..4 {
        key(&mut k, Key::Backspace);
    }
    assert!(k
        .render_state()
        .candidates
        .iter()
        .any(|c| c.language == CandidateLanguage::English));
    assert_eq!(committed(&key(&mut k, Key::Enter)), Some("hel"));
    input(&mut k, "can");
    assert_eq!(k.render_state().candidates[0].text, "can");
    assert!(k.render_state().candidates.iter().any(|c| c.text == "擦"));
}

#[test]
fn mixed_segment_selection_preserves_original_case_and_learns_only_committed_parts() {
    let mut k = kernel(PinyinScheme::Full);
    input(&mut k, "niHello");
    let index = k
        .render_state()
        .candidates
        .iter()
        .position(|c| c.text == "你")
        .unwrap();
    let partial = k.handle(InputEvent::CandidateChosen { index });
    assert!(!partial
        .iter()
        .any(|a| matches!(a, KernelAction::Side(SideEffect::Learn(_)))));
    assert_eq!(k.composition_text(), "你Hello");
    let index = k
        .render_state()
        .candidates
        .iter()
        .position(|c| c.text == "Hello")
        .unwrap();
    let actions = k.handle(InputEvent::CandidateChosen { index });
    assert_eq!(committed(&actions), Some("你Hello"));
    assert!(actions
        .iter()
        .any(|a| matches!(a, KernelAction::Side(SideEffect::Learn(
        LearningEvent::EnglishWord{text})) if text == "Hello")));
    assert!(actions
        .iter()
        .any(|a| matches!(a, KernelAction::Side(SideEffect::Learn(
        LearningEvent::CandidateChosen{text,..})) if text == "你")));
    let mut k = kernel(PinyinScheme::Flypy);
    k.handle(InputEvent::Key {
        key: Key::Char('h'),
        mods: Modifiers::SHIFT,
        source: InputSource::Keyboard,
    });
    input(&mut k, "ello");
    assert_eq!(k.composition_text(), "Hello");
    assert_eq!(k.render_state().candidates[0].text, "Hello");
    assert_eq!(committed(&key(&mut k, Key::Char(','))), Some("Hello，"));
}

#[test]
fn disabling_english_and_stale_corrections_do_not_change_chinese_candidates() {
    let mut k = kernel(PinyinScheme::Full);
    input(&mut k, "hello");
    let before = k.render_state();
    k.handle(InputEvent::EnglishCompleted {
        gen: k.generation(),
        candidates: vec![Candidate::new("incorrect", CandidateSource::Local)],
    });
    assert_eq!(k.render_state(), before);
    key(&mut k, Key::Escape);
    k.handle(InputEvent::SetEnglishOptions {
        enabled: false,
        spelling: true,
    });
    input(&mut k, "hello");
    assert!(!k
        .render_state()
        .candidates
        .iter()
        .any(|c| c.language == CandidateLanguage::English));
}
