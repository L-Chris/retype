//! retype 统一输入与联想内核。
//!
//! 对应 test.md 图 5：语音、物理键盘、触摸键盘共用同一个「大脑」。
//! 三种输入方式在这里只是 [`retype_types::InputSource`] 上的一个标签，
//! 它们共享同一份上下文、同一套候选、同一个用户词库、同一套学习记录。
//!
//! 结构：
//! - [`Kernel`] —— 纯状态机，不开线程、不做 IO，可完全单测
//! - [`KernelBackend`] —— 把状态机接上并发（[`LocalBackend`] / [`InlineBackend`]）
//! - [`merge`] —— P3 合并规则：二刷只能重排 + 追加
#![forbid(unsafe_code)]

pub mod backend;
pub mod kernel;
pub mod merge;

pub use backend::{BackendOptions, InlineBackend, KernelBackend, LocalBackend};
pub use kernel::{Kernel, KernelConfig, VoicePhase};
pub use merge::{apply_response, merge_outcome};

// 平台层从这里一次性拿到内核 + 上下文策略，避免各处 import 路径不一致
pub use retype_context::{ContextCollector, NullCollector, PrivacyPolicy};

use retype_cloud::{CloudClient, MockCloudPinyin, MockConfig, MockLlmReranker, UnavailableCloud};
use std::sync::Arc;
use std::time::Duration;

/// 二刷的默认超时。超过就放弃这次增强，首刷结果原样保留。
///
/// 取 800ms 的理由：用户连续打字时按键间隔常在 100~300ms，
/// 800ms 足够一次 LLM 往返，又不至于让积压的请求在停手后一起回来搅乱候选。
pub const DEFAULT_RERANK_TIMEOUT: Duration = Duration::from_millis(800);

/// 用 Mock 云端装配 `CloudClient`（开发 / 测试 / `retype-diag`）。
///
/// 生产装配（M3）会把 `MockLlmReranker` 换成真实供应商实现，其余不动 —— 见 ADR-0002。
pub fn mock_cloud(cfg: MockConfig, timeout: Duration) -> Arc<CloudClient> {
    Arc::new(CloudClient::new(
        Arc::new(MockLlmReranker::new(cfg.clone())),
        Arc::new(MockCloudPinyin::new(cfg)),
        timeout,
    ))
}

/// 「未配置云端」的装配：所有云端调用立即失败，输入法完全靠本地工作。
pub fn offline_cloud(timeout: Duration) -> Arc<CloudClient> {
    Arc::new(CloudClient::new(
        Arc::new(UnavailableCloud),
        Arc::new(UnavailableCloud),
        timeout,
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use retype_dict::{from_pairs, LayeredDict, Learner, MemoryDict, UserDict, DEFAULT_USER_BOOST};
    use retype_pinyin::Lexicon;
    use retype_types::{
        AppInfo, AsrEvent, Candidate, CandidateSource, CommitRequest, ContextSnapshot, FieldInfo,
        InputEvent, InputSource, KernelAction, Key, LearningEvent, LearningStore, Modifiers,
        PrivacyLevel, RenderState, RerankOutcome, SideEffect, StatusFlags, VoiceEvent,
    };
    use std::time::Instant;

    fn demo_dict() -> Arc<dyn Lexicon> {
        let (d, _) = from_pairs([
            ("你", "ni", 100000.0),
            ("好", "hao", 80000.0),
            ("你好", "ni hao", 6000.0),
            ("吗", "ma", 20000.0),
            ("世", "shi", 30000.0),
            ("界", "jie", 25000.0),
            ("世界", "shi jie", 9000.0),
            ("是", "shi", 90000.0),
            ("实力", "shi li", 5000.0),
            ("事例", "shi li", 4000.0),
            ("治理", "zhi li", 4500.0),
            ("上海", "shang hai", 7000.0),
            ("西", "xi", 28000.0),
            ("安", "an", 26000.0),
            ("西安", "xi an", 6500.0),
        ]);
        Arc::new(d)
    }

    /// 内核 + 分层词库（系统层 + 用户层），这样学习回写才能真的影响下一次候选。
    fn make_kernel(rerank: bool, mock: MockConfig) -> (Kernel, Arc<UserDict>, Arc<CloudClient>) {
        let user = Arc::new(UserDict::new());
        let sys = demo_dict();
        let learner: Arc<dyn LearningStore> =
            Arc::new(Learner::with_system(Arc::clone(&user), Arc::clone(&sys)));
        let dict: Arc<dyn Lexicon> = Arc::new(LayeredDict::with_system_and_user(
            sys,
            Arc::clone(&user),
            DEFAULT_USER_BOOST,
        ));
        let cloud = mock_cloud(mock, Duration::from_millis(500));
        let k = Kernel::new(
            KernelConfig {
                rerank_enabled: rerank,
                ..Default::default()
            },
            dict,
            learner,
            Arc::clone(&cloud),
        );
        (k, user, cloud)
    }

    struct Fixture {
        backend: InlineBackend,
        #[allow(dead_code)]
        user: Arc<UserDict>,
    }

    fn fixture(rerank: bool) -> Fixture {
        fixture_with(rerank, MockConfig::default())
    }

    #[test]
    fn flypy_zero_initial_prefix_selection_restores_original_keys() {
        let (dict, _) = from_pairs([
            ("额度", "e du", 1000.0),
            ("额度吗", "e du ma", 5000.0),
            ("吗", "ma", 2000.0),
        ]);
        let cloud = offline_cloud(Duration::from_millis(100));
        let kernel = Kernel::new(
            KernelConfig {
                pinyin_scheme: retype_types::PinyinScheme::Flypy,
                rerank_enabled: false,
                ..Default::default()
            },
            Arc::new(dict),
            Arc::new(Learner::new(Arc::new(UserDict::new()))),
            Arc::clone(&cloud),
        );
        let backend = InlineBackend::new(kernel, cloud);
        type_str(&backend, "edum");
        assert!(backend
            .render()
            .candidates
            .iter()
            .any(|c| c.text == "额度吗" && c.consumed == 4));
        type_str(&backend, "a");
        let render = backend.render();
        let index = render
            .candidates
            .iter()
            .position(|c| c.text == "额度")
            .expect("zero-initial prefix candidate");
        assert_eq!(render.candidates[index].consumed, 3);
        backend.submit(InputEvent::CandidateChosen { index });
        assert_eq!(backend.render().composition, "额度ma");
        for _ in 0..3 {
            backend.submit(key_ev(Key::Backspace));
        }
        assert_eq!(backend.render().composition, "edu");
        assert_eq!(
            committed_of(&backend.submit(key_ev(Key::Space))).as_deref(),
            Some("额度")
        );
    }

    #[test]
    fn flypy_commit_and_prefix_consumption_use_original_keys() {
        let f = fixture(false);
        f.backend.submit(InputEvent::SetPinyinScheme(
            retype_types::PinyinScheme::Flypy,
        ));
        type_str(&f.backend, "nihc");
        let render = f.backend.render();
        assert_eq!(render.candidates[0].text, "你好");
        assert_eq!(render.candidates[0].consumed, 4);
        let actions = f.backend.submit(key_ev(Key::Space));
        assert!(actions.iter().any(|a| matches!(a, KernelAction::Commit(CommitRequest::ReplaceComposition { text }) if text == "你好")));
        type_str(&f.backend, "nihcma");
        let render = f.backend.render();
        let index = render
            .candidates
            .iter()
            .position(|c| c.text == "你好")
            .expect("prefix candidate");
        assert_eq!(render.candidates[index].consumed, 4);
        f.backend.submit(InputEvent::CandidateChosen { index });
        assert_eq!(f.backend.render().composition, "你好ma");
        for _ in 0..3 {
            f.backend.submit(key_ev(Key::Backspace));
        }
        assert_eq!(f.backend.render().composition, "nihc");
        f.backend.submit(InputEvent::SetPinyinScheme(
            retype_types::PinyinScheme::Full,
        ));
        assert!(f.backend.render().composition.is_empty());
        type_str(&f.backend, "nihao");
        assert_eq!(f.backend.render().candidates[0].text, "你好");
    }

    #[test]
    fn flypy_syllable_boundary_cannot_be_resegmented_as_full_pinyin() {
        let f = fixture(false);
        f.backend.submit(InputEvent::SetPinyinScheme(
            retype_types::PinyinScheme::Flypy,
        ));
        type_str(&f.backend, "xm"); // xian is ONE syllable, never xi + an.
        assert!(!f
            .backend
            .render()
            .candidates
            .iter()
            .any(|c| c.text == "西安"));
        f.backend.submit(key_ev(Key::Escape));
        type_str(&f.backend, "xian"); // xi + an in Flypy.
        assert!(f
            .backend
            .render()
            .candidates
            .iter()
            .any(|c| c.text == "西安"));
        f.backend.submit(key_ev(Key::Escape));
        type_str(&f.backend, "ni'hc");
        assert_eq!(f.backend.render().candidates[0].text, "你好");
        assert_eq!(f.backend.render().candidates[0].consumed, 5);
    }

    #[test]
    fn flypy_third_key_refreshes_candidates_without_showing_raw_letters() {
        let (dict, _) = from_pairs([
            ("没", "mei", 1000.0),
            ("我", "wo", 2000.0),
            ("没有", "mei you", 5000.0),
            ("渥恩", "wo en", 1.0),
        ]);
        let cloud = offline_cloud(Duration::from_millis(100));
        let kernel = Kernel::new(
            KernelConfig {
                pinyin_scheme: retype_types::PinyinScheme::Flypy,
                rerank_enabled: false,
                ..Default::default()
            },
            Arc::new(dict),
            Arc::new(Learner::new(Arc::new(UserDict::new()))),
            Arc::clone(&cloud),
        );
        let backend = InlineBackend::new(kernel, cloud);
        type_str(&backend, "mwy");
        let render = backend.render();
        assert_eq!(render.composition, "mwy");
        assert_eq!(render.candidates[0].text, "没有");
        assert!(render.candidates.iter().all(|c| !c.text.contains('y')));
        assert_eq!(
            committed_of(&backend.submit(key_ev(Key::Space))).as_deref(),
            Some("没有")
        );

        type_str(&backend, "woe");
        let render = backend.render();
        assert_eq!(render.candidates[0].text, "我");
        assert!(render.candidates.iter().all(|c| !c.text.contains('e')));
        assert_eq!(
            committed_of(&backend.submit(key_ev(Key::Char(',')))).as_deref(),
            Some("我e，"),
            "typing punctuation must not discard the unmatched suffix"
        );
    }

    #[test]
    fn numeric_selection_does_not_choose_hidden_next_page() {
        // One item per page makes candidate 2 invisible even though it exists.
        let mut kernel = Kernel::new(
            KernelConfig {
                decode: retype_pinyin::DecodeOptions {
                    page_size: 1,
                    ..Default::default()
                },
                rerank_enabled: false,
                ..Default::default()
            },
            demo_dict(),
            Arc::new(Learner::new(Arc::new(UserDict::new()))),
            offline_cloud(Duration::from_millis(100)),
        );
        type_kernel(&mut kernel, "ni");
        let before = kernel.render_state().composition;
        let actions = kernel.handle(key_ev(Key::Char('2')));
        assert!(!actions.iter().any(|a| matches!(a, KernelAction::Commit(_))));
        assert_eq!(kernel.render_state().composition, before);
    }

    fn fixture_with(rerank: bool, mock: MockConfig) -> Fixture {
        let (k, user, cloud) = make_kernel(rerank, mock);
        Fixture {
            backend: InlineBackend::new(k, cloud),
            user,
        }
    }

    fn key_ev(k: Key) -> InputEvent {
        InputEvent::Key {
            key: k,
            mods: Modifiers::NONE,
            source: InputSource::Keyboard,
        }
    }

    fn type_str(b: &InlineBackend, s: &str) -> Vec<KernelAction> {
        let mut out = Vec::new();
        for c in s.chars() {
            out.extend(b.submit(key_ev(Key::Char(c))));
        }
        out
    }

    fn type_kernel(k: &mut Kernel, s: &str) {
        for c in s.chars() {
            k.handle(key_ev(Key::Char(c)));
        }
    }

    fn key(b: &InlineBackend, k: Key) -> Vec<KernelAction> {
        b.submit(key_ev(k))
    }

    fn last_render(acts: &[KernelAction]) -> RenderState {
        acts.iter()
            .rev()
            .find_map(|a| match a {
                KernelAction::Render(r) => Some(r.clone()),
                _ => None,
            })
            .expect("应该有一次渲染")
    }

    fn committed_of(acts: &[KernelAction]) -> Option<String> {
        acts.iter().find_map(|a| match a {
            KernelAction::Commit(CommitRequest::ReplaceComposition { text }) => Some(text.clone()),
            KernelAction::Commit(CommitRequest::Text(text)) => Some(text.clone()),
            _ => None,
        })
    }

    // ── 拼音主链路 ──────────────────────────────────────────────

    #[test]
    fn typing_produces_candidates_without_committing() {
        let f = fixture(false);
        let acts = type_str(&f.backend, "nihao");
        let r = last_render(&acts);
        assert_eq!(r.composition, "nihao");
        assert_eq!(r.converted_len, 0);
        assert_eq!(r.syllables, ["ni", "hao"]);
        assert!(r.candidates.iter().any(|c| c.text == "你好"));
        assert!(
            !acts.iter().any(|a| matches!(a, KernelAction::Commit(_))),
            "组字过程中不应该上屏"
        );
    }

    #[test]
    fn space_commits_first_candidate() {
        let f = fixture(false);
        type_str(&f.backend, "nihao");
        let acts = key(&f.backend, Key::Space);
        assert_eq!(committed_of(&acts).as_deref(), Some("你好"));
        assert!(
            last_render(&acts).composition.is_empty(),
            "上屏后组字区必须清空"
        );
    }

    #[test]
    fn choosing_a_prefix_keeps_the_rest_composing() {
        let f = fixture(false);
        type_str(&f.backend, "nihaoma");
        let idx = f
            .backend
            .with_kernel(|k| k.candidate_texts().iter().position(|t| *t == "你好"))
            .expect("应有前缀候选「你好」");
        let acts = f.backend.submit(InputEvent::CandidateChosen { index: idx });
        let r = last_render(&acts);
        assert_eq!(
            r.composition, "你好ma",
            "已选部分转为文字，剩余拼音留在组字区"
        );
        assert_eq!(r.converted_len, 2);
        assert!(
            !acts.iter().any(|a| matches!(a, KernelAction::Commit(_))),
            "还有剩余拼音时不应上屏"
        );
    }

    #[test]
    fn backspace_restores_pinyin_from_a_chosen_part() {
        let f = fixture(false);
        type_str(&f.backend, "nihaoma");
        let idx = f.backend.with_kernel(|k| {
            k.candidate_texts()
                .iter()
                .position(|t| *t == "你好")
                .unwrap()
        });
        f.backend.submit(InputEvent::CandidateChosen { index: idx });

        let acts = key(&f.backend, Key::Backspace);
        assert_eq!(last_render(&acts).composition, "你好m");

        let acts = key(&f.backend, Key::Backspace);
        assert_eq!(last_render(&acts).composition, "你好");

        // 关键：buffer 空了以后退格，应把已选词**还原成拼音**，而不是删掉用户选好的字
        let acts = key(&f.backend, Key::Backspace);
        let r = last_render(&acts);
        assert_eq!(r.composition, "nihao");
        assert_eq!(r.converted_len, 0);
    }

    #[test]
    fn enter_commits_raw_letters() {
        let f = fixture(false);
        type_str(&f.backend, "nihao");
        let acts = key(&f.backend, Key::Enter);
        assert_eq!(committed_of(&acts).as_deref(), Some("nihao"));
    }

    #[test]
    fn escape_discards_composition() {
        let f = fixture(false);
        type_str(&f.backend, "nihao");
        let acts = key(&f.backend, Key::Escape);
        assert!(
            !acts.iter().any(|a| matches!(a, KernelAction::Commit(_))),
            "Esc 是取消，不是上屏"
        );
        assert!(last_render(&acts).composition.is_empty());
    }

    #[test]
    fn punctuation_commits_best_with_chinese_mark() {
        let f = fixture(false);
        type_str(&f.backend, "nihao");
        let acts = key(&f.backend, Key::Char(','));
        assert_eq!(committed_of(&acts).as_deref(), Some("你好，"));
        assert!(
            !acts.iter().any(|a| matches!(a, KernelAction::PassThrough)),
            "中文标点不能再次交回宿主产生重复字符"
        );
    }

    #[test]
    fn idle_punctuation_follows_mode_and_pairs_quotes() {
        let f = fixture(false);
        for (key_char, expected) in [
            (',', "，"),
            ('.', "。"),
            ('?', "？"),
            ('"', "“"),
            ('"', "”"),
            ('\'', "‘"),
            ('\'', "’"),
        ] {
            let actions = key(&f.backend, Key::Char(key_char));
            assert_eq!(committed_of(&actions).as_deref(), Some(expected));
        }
        f.backend.submit(InputEvent::ToggleChinese);
        let actions = key(&f.backend, Key::Char(','));
        assert!(committed_of(&actions).is_none());
        assert!(actions
            .iter()
            .any(|a| matches!(a, KernelAction::PassThrough)));
    }

    #[test]
    fn digit_selects_from_the_current_page() {
        let f = fixture(false);
        type_str(&f.backend, "shili");
        let second = f
            .backend
            .with_kernel(|k| k.render_state().candidates.get(1).map(|c| c.text.clone()));
        let acts = key(&f.backend, Key::Char('2'));
        assert_eq!(committed_of(&acts), second);
    }

    #[test]
    fn digit_without_composition_passes_through() {
        let f = fixture(false);
        let acts = key(&f.backend, Key::Char('5'));
        assert!(acts.iter().all(|a| matches!(a, KernelAction::PassThrough)));
    }

    #[test]
    fn arrow_keys_move_selection() {
        let f = fixture(false);
        type_str(&f.backend, "shili");
        let r = last_render(&key(&f.backend, Key::Right));
        assert_eq!(r.selected, 1, "右方向键应移动高亮");
    }

    #[test]
    fn measured_pages_keep_navigation_and_number_selection_in_sync() {
        let (dict, _) = from_pairs([
            ("实力", "shi li", 5000.0),
            ("事例", "shi li", 4000.0),
            ("示例", "shi li", 3000.0),
        ]);
        let cloud = offline_cloud(Duration::from_millis(100));
        let user = Arc::new(UserDict::new());
        let kernel = Kernel::new(
            KernelConfig {
                rerank_enabled: false,
                ..Default::default()
            },
            Arc::new(dict),
            Arc::new(Learner::new(Arc::clone(&user))),
            Arc::clone(&cloud),
        );
        let f = Fixture {
            backend: InlineBackend::new(kernel, cloud),
            user,
        };
        type_str(&f.backend, "shili");
        let original = f.backend.with_kernel(|k| k.render_state());
        assert!(original.candidates.len() >= 3);
        let mut widths = vec![180; original.candidates.len()];
        widths[0] = 90;
        widths[1] = 90;
        f.backend.layout_candidates(&widths, 200, 2);
        let first = f.backend.with_kernel(|k| k.render_state());
        assert_eq!(first.visible().len(), 2);
        assert_eq!(first.page_starts[1], 2);
        assert_eq!(first.candidates, original.candidates);
        assert!(committed_of(&key(&f.backend, Key::Char('3'))).is_none());
        let next = last_render(&key(&f.backend, Key::Char('=')));
        assert_eq!(next.page_start, 2);
        assert_eq!(next.visible().len(), 1);
        let previous = last_render(&key(&f.backend, Key::Char('-')));
        assert_eq!(previous.page_start, 0);
        let selected = key(&f.backend, Key::Char('2'));
        assert_eq!(
            committed_of(&selected).as_deref(),
            Some(original.candidates[1].text.as_str())
        );
    }

    #[test]
    fn minus_and_equals_turn_pages_but_plus_is_punctuation() {
        let mut kernel = Kernel::new(
            KernelConfig {
                decode: retype_pinyin::DecodeOptions {
                    page_size: 1,
                    ..Default::default()
                },
                rerank_enabled: false,
                ..Default::default()
            },
            demo_dict(),
            Arc::new(Learner::new(Arc::new(UserDict::new()))),
            offline_cloud(Duration::from_millis(100)),
        );
        type_kernel(&mut kernel, "shili");
        let before = kernel.render_state();
        assert!(before.candidates.len() > before.page_size);
        let next = last_render(&kernel.handle(InputEvent::Key {
            key: Key::Char('='),
            mods: Modifiers::NONE,
            source: InputSource::Keyboard,
        }));
        assert_eq!(next.page_start, before.page_size);
        assert_eq!(next.composition, before.composition);
        let previous = last_render(&kernel.handle(key_ev(Key::Char('-'))));
        assert_eq!(previous.page_start, 0);
        assert_eq!(previous.composition, before.composition);

        let plus = kernel.handle(InputEvent::Key {
            key: Key::Char('+'),
            mods: Modifiers::SHIFT,
            source: InputSource::Keyboard,
        });
        assert!(committed_of(&plus).is_some());
        assert!(plus.iter().any(|a| matches!(a, KernelAction::PassThrough)));
    }

    #[test]
    fn apostrophe_forces_syllable_boundary() {
        let f = fixture(false);
        let acts = type_str(&f.backend, "xi'an");
        let r = last_render(&acts);
        assert_eq!(r.syllables, ["xi", "an"], "显式分隔符必须生效");
    }

    // ── 模式切换与焦点 ──────────────────────────────────────────

    #[test]
    fn english_mode_passes_everything_through() {
        let f = fixture(false);
        f.backend.submit(InputEvent::ToggleChinese);
        let acts = type_str(&f.backend, "nihao");
        assert!(acts.iter().all(|a| matches!(a, KernelAction::PassThrough)));
        assert!(f.backend.with_kernel(|k| !k.has_composition()));
    }

    #[test]
    fn toggling_while_composing_commits_letters_first() {
        let f = fixture(false);
        type_str(&f.backend, "nihao");
        let acts = f.backend.submit(InputEvent::ToggleChinese);
        assert!(matches!(
            acts.first(),
            Some(KernelAction::Commit(
                CommitRequest::ReplaceComposition { .. }
            ))
        ));
    }

    #[test]
    fn ctrl_shortcut_passes_through_and_drops_composition() {
        let f = fixture(false);
        type_str(&f.backend, "nihao");
        let acts = f.backend.submit(InputEvent::Key {
            key: Key::Char('c'),
            mods: Modifiers::CTRL,
            source: InputSource::Keyboard,
        });
        assert!(
            acts.contains(&KernelAction::PassThrough),
            "Ctrl+C 必须给宿主"
        );
        assert!(
            f.backend.with_kernel(|k| !k.has_composition()),
            "不能留着半截组字"
        );
        assert!(
            !acts.iter().any(|a| matches!(a, KernelAction::Commit(_))),
            "快捷键不该把拼音上屏"
        );
    }

    #[test]
    fn focus_change_discards_composition_instead_of_committing() {
        // test.md 第六节头号事故：文字插入到错误窗口
        let f = fixture(false);
        type_str(&f.backend, "nihao");
        let acts = f.backend.submit(InputEvent::FocusChanged {
            app: AppInfo::new("code.exe"),
            field: FieldInfo::default(),
        });
        assert!(
            !acts.iter().any(|a| matches!(a, KernelAction::Commit(_))),
            "切换焦点绝不能上屏"
        );
        assert!(last_render(&acts).composition.is_empty());
        assert!(
            acts.iter()
                .any(|a| matches!(a, KernelAction::Side(SideEffect::CollectContext))),
            "焦点变化应触发一次上下文采集"
        );
    }

    // ── 二刷（首刷 → 云端重排 → 合并）──────────────────────────

    #[test]
    fn rerank_reorders_using_context_without_losing_candidates() {
        let f = fixture(true);
        f.backend
            .submit(InputEvent::ContextUpdated(ContextSnapshot {
                text_before: "这是一个典型的事例".into(),
                privacy: PrivacyLevel::Cloud,
                ..Default::default()
            }));
        type_str(&f.backend, "shili");
        let r = f.backend.with_kernel(|k| k.render_state());
        let texts: Vec<&str> = r.candidates.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(
            texts.first().copied(),
            Some("事例"),
            "上下文相关的词应被二刷提前"
        );
        assert!(texts.contains(&"实力"), "但首刷的候选一个都不能少（P3）");
    }

    #[test]
    fn without_context_rerank_changes_nothing() {
        let f = fixture(true);
        type_str(&f.backend, "shili");
        let r = f.backend.with_kernel(|k| k.render_state());
        assert_eq!(
            r.candidates.first().map(|c| c.text.as_str()),
            Some("实力"),
            "无上下文时二刷不应打乱本地排序"
        );
    }

    #[test]
    fn rerank_disabled_still_gives_full_local_candidates() {
        let f = fixture(false);
        type_str(&f.backend, "shili");
        let r = f.backend.with_kernel(|k| k.render_state());
        assert!(r.candidates.len() >= 2);
        assert!(!r.status.contains(StatusFlags::CLOUD_BUSY));
    }

    #[test]
    fn stale_rerank_result_is_discarded() {
        let (mut k, _, _) = make_kernel(true, MockConfig::default());
        type_kernel(&mut k, "shili");
        let gen_now = k.generation();
        let before: Vec<String> = k
            .candidate_texts()
            .iter()
            .map(|s| (*s).to_owned())
            .collect();

        // 一个「上上代」的重排结果，顺序被完全颠倒
        let mut ranked: Vec<Candidate> = before
            .iter()
            .map(|t| Candidate::new(t.clone(), CandidateSource::Cloud))
            .collect();
        ranked.reverse();
        let acts = k.handle(InputEvent::RerankCompleted {
            gen: gen_now.saturating_sub(3),
            result: RerankOutcome {
                ranked,
                extra: Vec::new(),
                degraded: false,
            },
        });

        assert!(
            !acts.iter().any(|a| matches!(a, KernelAction::Render(_))),
            "过期结果必须整包丢弃，连重渲染都不该发生"
        );
        let after: Vec<String> = k
            .candidate_texts()
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(before, after, "候选顺序不能被过期结果改动");
    }

    #[test]
    fn degraded_rerank_keeps_first_pass_intact() {
        let f = fixture_with(
            true,
            MockConfig {
                fail: true,
                ..Default::default()
            },
        );
        type_str(&f.backend, "shili");
        let r = f.backend.with_kernel(|k| k.render_state());
        assert!(
            r.candidates.iter().any(|c| c.text == "实力"),
            "云端挂了也要有完整的首刷候选"
        );
        assert!(
            !r.status.contains(StatusFlags::CLOUD_BUSY),
            "不能卡在「云端处理中」"
        );
        assert!(
            !r.status.contains(StatusFlags::CLOUD_OK),
            "降级后状态栏的云图标应熄灭"
        );
    }

    #[test]
    fn selection_follows_the_word_across_a_rerank() {
        let (mut k, _, _) = make_kernel(false, MockConfig::default());
        type_kernel(&mut k, "shili");
        k.handle(key_ev(Key::Right));
        let r0 = k.render_state();
        let picked = r0.candidates[r0.selected].text.clone();
        let gen = k.generation();

        let mut ranked = r0.candidates.clone();
        ranked.reverse();
        for c in ranked.iter_mut() {
            c.source = CandidateSource::Cloud;
        }
        k.handle(InputEvent::RerankCompleted {
            gen,
            result: RerankOutcome {
                ranked,
                extra: Vec::new(),
                degraded: false,
            },
        });

        let r1 = k.render_state();
        assert_eq!(
            r1.candidates[r1.selected].text, picked,
            "重排后高亮必须跟着同一个词走"
        );
        assert_ne!(r1.candidates, r0.candidates, "顺序确实被改过了");
    }

    // ── 学习闭环 ────────────────────────────────────────────────

    #[test]
    fn repeated_choice_promotes_a_word() {
        let f = fixture(false);
        for round in 0..8 {
            type_str(&f.backend, "shili");
            let idx = f
                .backend
                .with_kernel(|k| k.candidate_texts().iter().position(|t| *t == "事例"))
                .unwrap_or(1);
            f.backend.submit(InputEvent::CandidateChosen { index: idx });
            if round == 0 {
                // 第一次选择之后不该立刻变天
                type_str(&f.backend, "shili");
                let r = f.backend.with_kernel(|k| k.render_state());
                assert_eq!(
                    r.candidates.first().map(|c| c.text.as_str()),
                    Some("实力"),
                    "一次选择不足以改变排序"
                );
                f.backend.submit(key_ev(Key::Escape));
            }
        }
        type_str(&f.backend, "shili");
        let r = f.backend.with_kernel(|k| k.render_state());
        assert_eq!(
            r.candidates.first().map(|c| c.text.as_str()),
            Some("事例"),
            "反复选同一个词后它应成为首选（test.md 图 7）"
        );
    }

    #[test]
    fn learning_is_dispatched_as_a_side_effect_not_done_inline() {
        // P1：学习落盘绝不能在输入线程上做
        let f = fixture(false);
        type_str(&f.backend, "nihao");
        let acts = f.backend.submit(InputEvent::CandidateChosen { index: 0 });
        assert!(
            !acts
                .iter()
                .any(|a| matches!(a, KernelAction::Side(SideEffect::Learn(_)))),
            "InlineBackend 已就地执行学习，不该再往外抛"
        );
        let (mut k, _, _) = make_kernel(false, MockConfig::default());
        type_kernel(&mut k, "nihao");
        let raw = k.handle(InputEvent::CandidateChosen { index: 0 });
        assert!(
            raw.iter().any(|a| matches!(
                a,
                KernelAction::Side(SideEffect::Learn(LearningEvent::CandidateChosen { .. }))
            )),
            "内核本身只投递学习事件，由后端决定在哪个线程执行"
        );
    }

    // ── 语音三段式 ──────────────────────────────────────────────

    fn voice(b: &InlineBackend, ev: VoiceEvent) -> Vec<KernelAction> {
        b.submit(InputEvent::Voice(ev))
    }

    #[test]
    fn voice_three_passes_end_with_final_commit() {
        let f = fixture(false);
        voice(&f.backend, VoiceEvent::Start);

        let a1 = voice(
            &f.backend,
            VoiceEvent::Asr(AsrEvent::Interim("今天天".into())),
        );
        assert_eq!(last_render(&a1).composition, "今天天");
        assert!(last_render(&a1)
            .status
            .contains(StatusFlags::VOICE_RECORDING));

        // 第二遍：停顿后前文被改写
        let a2 = voice(
            &f.backend,
            VoiceEvent::Asr(AsrEvent::Stable("今天天气不错".into())),
        );
        assert_eq!(last_render(&a2).composition, "今天天气不错");

        // 松手：final 还没到，必须进入「识别优化中」而不是立刻结束
        let a3 = voice(&f.backend, VoiceEvent::Stop);
        assert!(last_render(&a3)
            .status
            .contains(StatusFlags::VOICE_OPTIMIZING));
        assert!(
            !a3.iter().any(|a| matches!(a, KernelAction::Commit(_))),
            "松手不等于上屏，还要等 final pass"
        );
        assert_eq!(
            f.backend.with_kernel(|k| k.voice_phase()),
            VoicePhase::Optimizing
        );

        // 第三遍：整段定稿
        let a4 = voice(
            &f.backend,
            VoiceEvent::Asr(AsrEvent::Final("今天天气不错。".into())),
        );
        assert_eq!(committed_of(&a4).as_deref(), Some("今天天气不错。"));
        assert!(!last_render(&a4)
            .status
            .contains(StatusFlags::VOICE_OPTIMIZING));
        assert_eq!(f.backend.with_kernel(|k| k.voice_phase()), VoicePhase::Idle);
    }

    #[test]
    fn voice_commit_dispatches_a_learning_event() {
        // 内核只投递学习事件，由后端决定在哪个线程执行（P1）
        let (mut k, _, _) = make_kernel(false, MockConfig::default());
        k.handle(InputEvent::Voice(VoiceEvent::Start));
        k.handle(InputEvent::Voice(VoiceEvent::Asr(AsrEvent::Final(
            "奥司他韦".into(),
        ))));
        let acts = k.handle(InputEvent::Voice(VoiceEvent::Stop));
        assert!(
            acts.iter().any(|a| matches!(
                a,
                KernelAction::Side(SideEffect::Learn(LearningEvent::VoiceCommit { text }))
                    if text == "奥司他韦"
            )),
            "语音上屏的词要回写学习（test.md 图 5 闭环）"
        );
    }

    #[test]
    fn final_before_release_commits_immediately() {
        let f = fixture(false);
        voice(&f.backend, VoiceEvent::Start);
        voice(&f.backend, VoiceEvent::Asr(AsrEvent::Final("好了".into())));
        let acts = voice(&f.backend, VoiceEvent::Stop);
        assert_eq!(
            committed_of(&acts).as_deref(),
            Some("好了"),
            "final 已到就不该再无谓地显示「优化中」"
        );
        assert!(!last_render(&acts)
            .status
            .contains(StatusFlags::VOICE_OPTIMIZING));
    }

    #[test]
    fn optimize_timeout_falls_back_to_stable() {
        // test.md 硬要求：超时也要上屏，绝不让用户的字消失
        let f = fixture(false);
        voice(&f.backend, VoiceEvent::Start);
        voice(
            &f.backend,
            VoiceEvent::Asr(AsrEvent::Stable("稳定的那段".into())),
        );
        voice(&f.backend, VoiceEvent::Stop);
        let acts = voice(&f.backend, VoiceEvent::OptimizeTimeout);
        assert_eq!(committed_of(&acts).as_deref(), Some("稳定的那段"));
        assert!(!last_render(&acts)
            .status
            .contains(StatusFlags::VOICE_OPTIMIZING));
    }

    #[test]
    fn timeout_without_stable_uses_interim() {
        let f = fixture(false);
        voice(&f.backend, VoiceEvent::Start);
        voice(
            &f.backend,
            VoiceEvent::Asr(AsrEvent::Interim("只有临时结果".into())),
        );
        voice(&f.backend, VoiceEvent::Stop);
        let acts = voice(&f.backend, VoiceEvent::OptimizeTimeout);
        assert_eq!(
            committed_of(&acts).as_deref(),
            Some("只有临时结果"),
            "连稳定段都没有时也要把临时结果救回来"
        );
    }

    #[test]
    fn late_interim_does_not_override_final_commit() {
        let f = fixture(false);
        voice(&f.backend, VoiceEvent::Start);
        voice(
            &f.backend,
            VoiceEvent::Asr(AsrEvent::Stable("定稿前的内容".into())),
        );
        voice(&f.backend, VoiceEvent::Stop);
        let acts = voice(
            &f.backend,
            VoiceEvent::Asr(AsrEvent::Final("最终定稿".into())),
        );
        assert_eq!(committed_of(&acts).as_deref(), Some("最终定稿"));
        // final 之后再来一个迟到的 interim，不许再上屏一次
        let late = voice(
            &f.backend,
            VoiceEvent::Asr(AsrEvent::Interim("迟到的临时结果".into())),
        );
        assert!(
            !late.iter().any(|a| matches!(a, KernelAction::Commit(_))),
            "会话已结束，迟到的 interim 只能更新预览"
        );
    }

    #[test]
    fn voice_cancel_commits_nothing() {
        let f = fixture(false);
        voice(&f.backend, VoiceEvent::Start);
        voice(
            &f.backend,
            VoiceEvent::Asr(AsrEvent::Stable("说错了".into())),
        );
        let acts = voice(&f.backend, VoiceEvent::Cancel);
        assert!(!acts.iter().any(|a| matches!(a, KernelAction::Commit(_))));
        assert!(last_render(&acts).composition.is_empty());
        assert_eq!(f.backend.with_kernel(|k| k.voice_phase()), VoicePhase::Idle);
    }

    #[test]
    fn silence_commits_nothing() {
        let f = fixture(false);
        voice(&f.backend, VoiceEvent::Start);
        let acts = voice(&f.backend, VoiceEvent::Stop);
        assert!(!acts.iter().any(|a| matches!(a, KernelAction::Commit(_))));
        assert_eq!(f.backend.with_kernel(|k| k.voice_phase()), VoicePhase::Idle);
    }

    #[test]
    fn starting_voice_drops_pending_pinyin() {
        let f = fixture(false);
        type_str(&f.backend, "nihao");
        let acts = voice(&f.backend, VoiceEvent::Start);
        let r = last_render(&acts);
        assert!(
            !r.composition.contains("nihao"),
            "语音和拼音不能同时占着组字区"
        );
    }

    // ── 降级 ────────────────────────────────────────────────────

    #[test]
    fn empty_dict_keeps_raw_letters_out_of_candidates() {
        let user = Arc::new(UserDict::new());
        let learner: Arc<dyn LearningStore> = Arc::new(Learner::new(Arc::clone(&user)));
        let empty: Arc<dyn Lexicon> = Arc::new(MemoryDict::default());
        let cloud = offline_cloud(Duration::from_millis(20));
        let k = Kernel::new(
            KernelConfig {
                rerank_enabled: false,
                ..Default::default()
            },
            empty,
            learner,
            Arc::clone(&cloud),
        );
        let b = InlineBackend::new(k, cloud);
        let acts = type_str(&b, "nihao");
        let r = last_render(&acts);
        assert!(r.candidates.is_empty(), "原样字母只留在组字串中");
        assert_eq!(r.composition, "nihao");
        assert!(r.status.contains(StatusFlags::DEGRADED), "并且要标记降级");
        assert_eq!(committed_of(&key(&b, Key::Enter)).as_deref(), Some("nihao"));
    }

    #[test]
    fn no_cloud_configured_still_types_fine() {
        let (k, _, _) = make_kernel(true, MockConfig::default());
        let cloud = offline_cloud(Duration::from_millis(20));
        let b = InlineBackend::new(k, cloud);
        // 把内核的云端换成「未配置」，模拟用户没填任何凭据
        let acts = type_str(&b, "nihaomashijie");
        let r = last_render(&acts);
        assert!(
            r.candidates.iter().any(|c| c.text == "你好吗世界"),
            "断网/未配置云端时必须能靠本地打完整句，实际 {:?}",
            r.candidates
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>()
        );
    }

    // ── 异步后端 ────────────────────────────────────────────────

    #[test]
    fn partial_choices_learn_only_when_committed_and_punctuation_accepts_the_default() {
        let (mut kernel, _, _) = make_kernel(false, MockConfig::default());
        type_kernel(&mut kernel, "nihaoma");
        let index = kernel
            .render_state()
            .candidates
            .iter()
            .position(|candidate| candidate.text == "你好")
            .expect("prefix candidate");
        let selected = kernel.handle(InputEvent::CandidateChosen { index });
        assert!(!selected
            .iter()
            .any(|action| matches!(action, KernelAction::Side(SideEffect::Learn(_)))));
        let committed = kernel.handle(key_ev(Key::Space));
        let learned: Vec<_> = committed
            .iter()
            .filter_map(|action| match action {
                KernelAction::Side(SideEffect::Learn(LearningEvent::CandidateChosen {
                    text,
                    ..
                })) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(learned, ["你好", "吗"]);
        type_kernel(&mut kernel, "nihaoma");
        let index = kernel
            .render_state()
            .candidates
            .iter()
            .position(|candidate| candidate.text == "你好")
            .expect("prefix candidate");
        kernel.handle(InputEvent::CandidateChosen { index });
        let cancelled = kernel.handle(key_ev(Key::Escape));
        assert!(!cancelled
            .iter()
            .any(|action| matches!(action, KernelAction::Side(SideEffect::Learn(_)))));
        type_kernel(&mut kernel, "nihao");
        let punctuation = kernel.handle(key_ev(Key::Char('.')));
        assert_eq!(
            punctuation
                .iter()
                .filter(|action| matches!(action, KernelAction::Side(SideEffect::Learn(_))))
                .count(),
            1
        );
    }

    #[test]
    fn deferred_learning_requires_successful_platform_commit_and_respects_privacy() {
        let (kernel, user, cloud) = make_kernel(false, MockConfig::default());
        let backend = LocalBackend::new(kernel, cloud);
        for ch in "nihao".chars() {
            backend.submit(key_ev(Key::Char(ch)));
        }
        let actions =
            backend.submit_deferred_learning(InputEvent::CandidateChosen { index: 0 }, true);
        let event = actions
            .into_iter()
            .find_map(|action| match action {
                KernelAction::Side(SideEffect::Learn(event)) => Some(event),
                _ => None,
            })
            .expect("learning should be deferred until the host commit succeeds");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(
            user.entry_count(),
            0,
            "an unconfirmed host write must not learn"
        );
        backend.record_learning(event);
        let deadline = Instant::now() + Duration::from_secs(2);
        while user.entry_count() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(user.entry_count(), 1);
        user.clear();
        for ch in "nihao".chars() {
            backend.submit(key_ev(Key::Char(ch)));
        }
        let actions =
            backend.submit_deferred_learning(InputEvent::CandidateChosen { index: 0 }, false);
        assert!(!actions
            .iter()
            .any(|action| matches!(action, KernelAction::Side(SideEffect::Learn(_)))));
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(
            user.entry_count(),
            0,
            "hidden input contexts must not learn"
        );
    }

    #[test]
    fn local_backend_returns_immediately_and_delivers_rerank_later() {
        let (k, _, cloud) = make_kernel(
            true,
            MockConfig {
                latency: Duration::from_millis(200),
                ..Default::default()
            },
        );
        let b = LocalBackend::with_options(
            k,
            cloud,
            BackendOptions {
                rerank_debounce: Duration::from_millis(20),
            },
        );

        // submit 必须立刻返回，不能被 200ms 的云端延迟拖住（P1）
        let start = Instant::now();
        for c in "shili".chars() {
            b.submit(key_ev(Key::Char(c)));
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(50),
            "首刷路径被云端延迟阻塞了：{elapsed:?}"
        );

        // 二刷结果异步到达
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got_render = false;
        while Instant::now() < deadline && !got_render {
            while let Some(a) = b.poll_action() {
                if matches!(a, KernelAction::Render(_)) {
                    got_render = true;
                }
            }
            if !got_render {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert!(got_render, "二刷完成后应通过 poll_action 收到一次重渲染");
    }

    #[test]
    fn debounce_collapses_a_burst_of_keystrokes_into_one_request() {
        // 连续打字时不该每个按键都发一次二刷：前面那些必然过期，白发只会拖慢一切
        let user = Arc::new(UserDict::new());
        let learner: Arc<dyn LearningStore> = Arc::new(Learner::new(Arc::clone(&user)));
        let mock = Arc::new(MockLlmReranker::new(MockConfig {
            latency: Duration::from_millis(5),
            ..Default::default()
        }));
        let cloud = Arc::new(CloudClient::new(
            Arc::clone(&mock) as Arc<dyn retype_cloud::LlmReranker>,
            Arc::new(MockCloudPinyin::new(MockConfig::default())),
            Duration::from_secs(2),
        ));
        let k = Kernel::new(
            KernelConfig::default(),
            demo_dict(),
            learner,
            Arc::clone(&cloud),
        );
        let b = LocalBackend::with_options(
            k,
            cloud,
            BackendOptions {
                rerank_debounce: Duration::from_millis(200),
            },
        );

        for c in "nihaomashijie".chars() {
            b.submit(key_ev(Key::Char(c)));
        }
        assert_eq!(
            mock.call_count(),
            0,
            "打字过程中一个二刷请求都不该发出去（去抖窗口还没过）"
        );

        // 停手后应发出**少量**请求，而不是 13 个
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && mock.call_count() == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(300));
        while b.poll_action().is_some() {}

        let calls = mock.call_count();
        assert!(calls >= 1, "停手后应至少发出一次二刷");
        assert!(calls <= 3, "13 次按键不该产生 13 次请求，实际 {calls} 次");

        let r = b.with_kernel(|k| k.render_state());
        assert!(
            r.candidates.iter().any(|c| c.text == "你好吗世界"),
            "去抖不能丢掉最终态的候选"
        );
    }

    #[test]
    fn local_backend_applies_learning_off_thread() {
        let (k, user, cloud) = make_kernel(false, MockConfig::default());
        let b = LocalBackend::new(k, cloud);
        for c in "nihao".chars() {
            b.submit(key_ev(Key::Char(c)));
        }
        b.submit(InputEvent::CandidateChosen { index: 0 });

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && user.entry_count() == 0 {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(user.entry_count() > 0, "选词学习应异步落到用户词库");
    }
}
