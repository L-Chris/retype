#![allow(clippy::unwrap_used, clippy::expect_used)]
use retype_dict::{annotate, from_pairs, LayeredDict, Learner, UserDict};
use retype_pinyin::{Decoder, Lexicon};
use retype_types::{Candidate, InputSource, LearningEvent, LearningStore};
use std::sync::Arc;

fn choice(learner: &Learner, text: &str, reading: &str, index: usize) {
    learner.record(LearningEvent::CandidateChosen {
        source: InputSource::Keyboard,
        text: text.into(),
        syllables: annotate::parse_pinyin(reading).unwrap(),
        index,
    });
}
fn fixture() -> (Arc<UserDict>, Learner, LayeredDict) {
    let base: Arc<dyn Lexicon> = Arc::new(
        from_pairs([
            ("实力", "shi li", 5000.),
            ("事例", "shi li", 4000.),
            ("其他", "qi ta", 100000.),
            ("重", "zhong", 5000.),
            ("众", "zhong", 4000.),
            ("重", "chong", 5000.),
            ("虫", "chong", 4000.),
        ])
        .0,
    );
    let user = Arc::new(UserDict::new());
    let learner = Learner::with_system(Arc::clone(&user), Arc::clone(&base));
    let lex = LayeredDict::with_system_and_user(base, Arc::clone(&user), 0.);
    (user, learner, lex)
}
fn decode(input: &str, lex: &dyn Lexicon) -> Vec<Candidate> {
    Decoder::new().decode(input, lex).candidates
}

#[test]
fn repeated_choices_change_preference_and_another_preference_can_replace_it() {
    let (_, learner, lex) = fixture();
    assert_eq!(decode("shili", &lex)[0].text, "实力");
    choice(&learner, "事例", "shi li", 20);
    assert_eq!(decode("shili", &lex)[0].text, "实力");
    for _ in 0..7 {
        choice(&learner, "事例", "shi li", 0);
    }
    assert_eq!(decode("shili", &lex)[0].text, "事例");
    for _ in 0..24 {
        choice(&learner, "实力", "shi li", 1);
    }
    assert_eq!(decode("shili", &lex)[0].text, "实力");
}

#[test]
fn recent_preference_decays_with_activity_without_erasing_lifetime_counts() {
    let (user, learner, lex) = fixture();
    for _ in 0..8 {
        choice(&learner, "事例", "shi li", 1);
    }
    assert_eq!(decode("shili", &lex)[0].text, "事例");
    for _ in 0..512 {
        choice(&learner, "其他", "qi ta", 0);
    }
    assert_eq!(decode("shili", &lex)[0].text, "实力");
    let history = user.usage_snapshot();
    let old = history
        .records
        .iter()
        .find(|record| record.text == "事例")
        .unwrap();
    assert_eq!(old.count, 8);
    assert!(old.recent_at(history.tick) < 1.);
}

#[test]
fn candidate_position_has_no_effect_on_learning() {
    let (a, learner_a, lex_a) = fixture();
    let (b, learner_b, lex_b) = fixture();
    for _ in 0..8 {
        choice(&learner_a, "事例", "shi li", 0);
        choice(&learner_b, "事例", "shi li", 100);
    }
    assert_eq!(a.usage_snapshot(), b.usage_snapshot());
    assert_eq!(decode("shili", &lex_a), decode("shili", &lex_b));
}

#[test]
fn learning_does_not_change_other_readings_or_their_score_mass() {
    let (_, learner, lex) = fixture();
    let unrelated = decode("zhong", &lex);
    let original = decode("shili", &lex);
    let mass = |rows: &[Candidate]| {
        rows.iter()
            .filter(|c| c.syllable_len == 2)
            .map(|c| (c.score as f64).exp())
            .sum::<f64>()
    };
    for _ in 0..12 {
        choice(&learner, "事例", "shi li", 3);
    }
    let changed = decode("shili", &lex);
    assert!((mass(&changed) / mass(&original) - 1.).abs() < 1e-6);
    choice(&learner, "重", "chong", 1);
    assert_eq!(
        decode("zhong", &lex),
        unrelated,
        "polyphonic readings must remain separate"
    );
}

#[test]
fn full_and_flypy_odd_and_even_codes_share_the_same_preference() {
    let (_, learner, lex) = fixture();
    for _ in 0..12 {
        choice(&learner, "事例", "shi li", 2);
    }
    for raw in ["uili", "uil"] {
        let result = retype_pinyin::shuangpin::decode(raw, &lex, &Default::default());
        let preferred = result
            .candidates
            .iter()
            .position(|c| c.text == "事例")
            .unwrap();
        let other = result
            .candidates
            .iter()
            .position(|c| c.text == "实力")
            .unwrap();
        assert!(preferred < other, "{raw}: {:?}", result.candidates);
    }
    assert_eq!(decode("shili", &lex)[0].text, "事例");
}

#[test]
fn registered_compound_becomes_a_whole_word_without_a_length_bonus() {
    let base: Arc<dyn Lexicon> = Arc::new(
        from_pairs([
            ("你", "ni", 1000.),
            ("拟", "ni", 800.),
            ("好", "hao", 1000.),
            ("号", "hao", 800.),
            ("参考", "can kao", 100000.),
        ])
        .0,
    );
    let user = Arc::new(UserDict::new());
    let learner = Learner::with_system(Arc::clone(&user), Arc::clone(&base));
    let lex = LayeredDict::with_system_and_user(base, Arc::clone(&user), 0.);
    let before = decode("nihao", &lex);
    let original = before.iter().find(|c| c.text == "拟号").unwrap().score;
    choice(&learner, "拟号", "ni hao", 3);
    let prior = user.usage_snapshot().records[0].prior_logp.unwrap();
    assert!((prior + retype_pinyin::DecodeOptions::default().word_bonus - original).abs() < 1e-5);
    // Once registered, the user phrase is a whole-word match. With no system
    // whole word for this reading, automatic compositions are now the fallback.
    let registered = decode("nihao", &lex);
    assert_eq!(registered[0].text, "拟号");
    assert!(!registered.iter().any(|candidate| candidate.text == "你好"));
    for _ in 0..12 {
        choice(&learner, "拟号", "ni hao", 0);
    }
    assert_eq!(decode("nihao", &lex)[0].text, "拟号");
}
