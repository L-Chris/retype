use retype_engine::{offline_cloud, Kernel, KernelConfig};
use retype_types::*;
use std::{sync::Arc, time::Duration};
fn kernel() -> Kernel {
    let user = Arc::new(retype_dict::UserDict::new());
    Kernel::new(
        KernelConfig {
            chinese_on_start: false,
            rerank_enabled: false,
            ..Default::default()
        },
        Arc::new(retype_dict::single_char_fallback()),
        Arc::new(retype_dict::Learner::new(user)),
        offline_cloud(Duration::from_millis(10)),
    )
}
fn key(k: &mut Kernel, key: Key, mods: Modifiers) -> Vec<KernelAction> {
    k.handle(InputEvent::Key {
        key,
        mods,
        source: InputSource::Keyboard,
    })
}
fn type_word(k: &mut Kernel, word: &str) {
    for c in word.chars() {
        key(k, Key::Char(c), Modifiers::NONE);
    }
}
fn commits(actions: &[KernelAction]) -> Vec<String> {
    actions
        .iter()
        .filter_map(|a| match a {
            KernelAction::Commit(
                CommitRequest::Text(t) | CommitRequest::ReplaceComposition { text: t },
            ) => Some(t.clone()),
            _ => None,
        })
        .collect()
}
#[test]
fn space_preserves_raw_and_tab_accepts_suggestion() {
    let mut k = kernel();
    type_word(&mut k, "hel");
    assert!(k
        .render_state()
        .candidates
        .iter()
        .any(|c| c.text == "hello"));
    assert!(!k.english_candidate_selected());
    assert_eq!(commits(&key(&mut k, Key::Space, Modifiers::NONE)), ["hel "]);
    type_word(&mut k, "hel");
    let first = k.render_state().candidates[0].text.clone();
    assert_eq!(
        commits(&key(&mut k, Key::Tab, Modifiers::NONE)),
        [format!("{first} ")]
    );
    assert!(!k.has_composition());
}

#[test]
fn switching_to_chinese_commits_and_learns_the_english_word() {
    let mut k = kernel();
    type_word(&mut k, "retype");
    let actions = k.handle(InputEvent::ToggleChinese);
    assert_eq!(commits(&actions), ["retype"]);
    assert!(actions.iter().any(|action| matches!(action,
        KernelAction::Side(SideEffect::Learn(LearningEvent::EnglishWord { text })) if text == "retype")));
    assert!(k.is_chinese());
    assert!(!k.has_composition());
}
#[test]
fn explicit_selection_space_and_native_enter() {
    let mut k = kernel();
    type_word(&mut k, "hel");
    key(&mut k, Key::Down, Modifiers::NONE);
    let first = k.render_state().candidates[0].text.clone();
    assert_eq!(
        commits(&key(&mut k, Key::Space, Modifiers::NONE)),
        [format!("{first} ")]
    );
    type_word(&mut k, "hel");
    let actions = key(&mut k, Key::Enter, Modifiers::NONE);
    assert_eq!(commits(&actions), ["hel"]);
    assert!(actions.contains(&KernelAction::PassThrough));
    assert!(actions.iter().any(|a| matches!(a,KernelAction::Side(SideEffect::Learn(LearningEvent::EnglishWord{text})) if text=="hel")));
    type_word(&mut k, "hel");
    key(&mut k, Key::Down, Modifiers::NONE);
    let actions = key(&mut k, Key::Enter, Modifiers::NONE);
    assert_eq!(commits(&actions), [first]);
    assert!(actions.contains(&KernelAction::PassThrough));
}

#[test]
fn choosing_a_word_adds_one_space_and_learns_only_the_word() {
    let mut k = kernel();
    type_word(&mut k, "hel");
    let word = k.render_state().candidates[0].text.clone();
    let actions = k.handle(InputEvent::CandidateChosen { index: 0 });
    assert_eq!(commits(&actions), [format!("{word} ")]);
    assert!(actions.iter().any(|action| matches!(action,
        KernelAction::Side(SideEffect::Learn(LearningEvent::EnglishWord { text })) if text == &word)));
    type_word(&mut k, "world");
    assert_eq!(k.render_state().composition, "world");
}
#[test]
fn case_apostrophes_punctuation_and_shortcuts_preserve_text() {
    let mut k = kernel();
    key(&mut k, Key::Char('H'), Modifiers::SHIFT);
    type_word(&mut k, "el");
    assert!(k
        .render_state()
        .candidates
        .iter()
        .any(|c| c.text == "Hello"));
    assert_eq!(
        commits(&key(&mut k, Key::Char('='), Modifiers::NONE)),
        ["Hel"]
    );
    type_word(&mut k, "don't");
    assert_eq!(
        commits(&key(&mut k, Key::Space, Modifiers::NONE)),
        ["don't "]
    );
    for ending in [Key::Char('4'), Key::Char('-'), Key::Left] {
        type_word(&mut k, "test");
        let actions = key(&mut k, ending, Modifiers::NONE);
        assert_eq!(commits(&actions), ["test"]);
        assert!(actions.contains(&KernelAction::PassThrough));
    }
    type_word(&mut k, "word");
    let actions = key(&mut k, Key::Char('a'), Modifiers::CTRL);
    assert_eq!(commits(&actions), ["word"]);
    assert!(actions.contains(&KernelAction::PassThrough));
}
#[test]
fn escape_keeps_text_reset_clears_and_late_spelling_is_ignored() {
    let mut k = kernel();
    type_word(&mut k, "hellp");
    let gen = k.generation();
    let candidates = retype_english::correct("hellp", 8);
    k.handle(InputEvent::EnglishCompleted {
        gen,
        candidates: candidates.clone(),
    });
    assert!(k
        .render_state()
        .candidates
        .iter()
        .any(|c| c.text == "hello"));
    assert_eq!(
        commits(&key(&mut k, Key::Escape, Modifiers::NONE)),
        ["hellp"]
    );
    assert!(k
        .handle(InputEvent::EnglishCompleted { gen, candidates })
        .is_empty());
    type_word(&mut k, "hello");
    k.handle(InputEvent::ResetComposition);
    assert!(!k.has_composition());
    type_word(&mut k, "hel");
    key(&mut k, Key::Backspace, Modifiers::NONE);
    assert_eq!(k.composition_text(), "he");
}
