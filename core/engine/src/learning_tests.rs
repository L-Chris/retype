#![allow(clippy::unwrap_used, clippy::expect_used)]
use crate::{offline_cloud, InlineBackend, Kernel, KernelBackend, KernelConfig};
use retype_dict::{annotate, from_pairs, LayeredDict, Learner, UserDict};
use retype_pinyin::Lexicon;
use retype_types::*;
use std::{sync::Arc, time::Duration};

fn backend(scheme: PinyinScheme) -> (InlineBackend, Arc<UserDict>, Arc<Learner>) {
    let system: Arc<dyn Lexicon> = Arc::new(
        from_pairs([
            ("啦去", "la qu", 10000.),
            ("拉取", "la qu", 1347.),
            ("啦", "la", 181128.),
            ("拉", "la", 177106.),
            ("去", "qu", 1049335.),
            ("取", "qu", 50000.),
        ])
        .0,
    );
    let user = Arc::new(UserDict::new());
    let learner = Arc::new(Learner::with_system(Arc::clone(&user), Arc::clone(&system)));
    let lexicon = Arc::new(LayeredDict::with_system_and_user(
        system,
        Arc::clone(&user),
        0.,
    ));
    let cloud = offline_cloud(Duration::from_millis(1));
    let kernel = Kernel::new(
        KernelConfig {
            pinyin_scheme: scheme,
            rerank_enabled: false,
            ..Default::default()
        },
        lexicon,
        Arc::clone(&learner) as Arc<dyn LearningStore>,
        Arc::clone(&cloud),
    );
    (InlineBackend::new(kernel, cloud), user, learner)
}
fn key(key: Key) -> InputEvent {
    InputEvent::Key {
        key,
        mods: Modifiers::NONE,
        source: InputSource::Keyboard,
    }
}
fn input(backend: &InlineBackend, spelling: &str) {
    for c in spelling.chars() {
        backend.submit(key(Key::Char(c)));
    }
}
fn choose(backend: &InlineBackend, text: &str) {
    let index = backend
        .render()
        .candidates
        .iter()
        .position(|c| c.text == text)
        .expect("candidate exists");
    backend.submit(InputEvent::CandidateChosen { index });
}
#[test]
fn segmented_la_qu_learns_the_phrase_in_both_schemes() {
    for scheme in [PinyinScheme::Full, PinyinScheme::Flypy] {
        let (backend, user, _) = backend(scheme);
        input(&backend, "laqu");
        assert_eq!(backend.render().candidates[0].text, "啦去");
        for _ in 0..48 {
            choose(&backend, "拉");
            choose(&backend, "取");
            input(&backend, "laqu");
        }
        assert_eq!(backend.render().candidates[0].text, "拉取");
        let usage = user.usage_snapshot();
        let phrase = usage
            .records
            .iter()
            .find(|r| r.text == "拉取")
            .expect("phrase learned");
        assert_eq!(phrase.syllables, annotate::parse_pinyin("la qu").unwrap());
        assert_eq!(phrase.count, 48);
    }
}
#[test]
fn punctuation_learns_the_selected_prefix_and_final_candidate_as_one_phrase() {
    let (backend, user, _) = backend(PinyinScheme::Full);
    input(&backend, "laqu");
    choose(&backend, "拉");
    let final_word = backend.render().candidates[0].text.clone();
    let actions = backend.submit(key(Key::Char('.')));
    assert!(actions.iter().any(|a| matches!(a, KernelAction::Commit(CommitRequest::ReplaceComposition{text}) if text == &format!("拉{final_word}。"))));
    assert!(user
        .usage_snapshot()
        .records
        .iter()
        .any(|r| r.text == format!("拉{final_word}") && r.count == 1));
}
#[test]
fn cancelled_and_undone_parts_do_not_teach_a_phrase() {
    let (backend, user, _) = backend(PinyinScheme::Full);
    input(&backend, "laqu");
    choose(&backend, "拉");
    backend.submit(key(Key::Escape));
    assert!(user.usage_snapshot().records.is_empty());
    input(&backend, "laqu");
    choose(&backend, "拉");
    backend.submit(key(Key::Backspace));
    backend.submit(key(Key::Backspace));
    backend.submit(key(Key::Backspace)); // Undo the selected 拉.
    choose(&backend, "啦");
    assert!(!user
        .usage_snapshot()
        .records
        .iter()
        .any(|r| r.text == "拉取"));
}

#[test]
fn personal_latin_spelling_does_not_displace_chinese_candidates() {
    let (backend, _, learner) = backend(PinyinScheme::Full);
    for _ in 0..2 {
        learner.record(LearningEvent::EnglishWord {
            text: "laqu".into(),
        });
    }
    input(&backend, "laqu");
    let candidates = backend.render().candidates;
    assert_eq!(candidates[0].text, "啦去");
    let raw = candidates.iter().position(|c| c.text == "laqu");
    let chinese = candidates
        .iter()
        .rposition(|c| c.language == CandidateLanguage::Chinese)
        .unwrap();
    assert!(
        raw.is_none_or(|index| index > chinese),
        "An unrecognized personal spelling must not take the second Chinese slot: {candidates:?}"
    );
    backend.submit(key(Key::Escape));
    backend.submit(InputEvent::ToggleChinese);
    input(&backend, "laqu");
    assert!(backend
        .render()
        .candidates
        .iter()
        .any(|c| c.text == "laqu" && c.language == CandidateLanguage::English));
}

#[test]
fn direct_whole_word_choices_record_the_full_reading_and_change_ranking() {
    for scheme in [PinyinScheme::Full, PinyinScheme::Flypy] {
        let (backend, user, _) = backend(scheme);
        input(&backend, "laqu");
        assert_eq!(backend.render().candidates[0].text, "啦去");
        for _ in 0..48 {
            choose(&backend, "拉取");
            input(&backend, "laqu");
        }
        let history = user.usage_snapshot();
        assert_eq!(
            history.records.len(),
            1,
            "Whole-word selection must not become single-character or English learning"
        );
        assert_eq!(history.records[0].text, "拉取");
        assert_eq!(
            history.records[0].syllables,
            annotate::parse_pinyin("la qu").unwrap()
        );
        assert_eq!(history.records[0].count, 48);
        assert_eq!(backend.render().candidates[0].text, "拉取");
    }
}
