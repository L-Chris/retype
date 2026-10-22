//! Android session adapter. Kotlin owns editor epochs and serializes JNI calls.
//! SQLite runs on a separate writer; learning is released only after host ACK.
use retype_dict::user::Layer;
use retype_dict::{LayeredDict, Learner, UserDict};
use retype_engine::{offline_cloud, Kernel, KernelConfig};
use retype_learning_store::{
    protocol::{Event, Request, SequencedEvent, VERSION},
    store::Store,
};
use retype_pinyin::Lexicon;
use retype_types::*;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::Mutex,
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};
pub mod features;
static LEARNING_IO: Mutex<()> = Mutex::new(());

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Command {
    VoiceLearn {
        text: String,
    },
    EnglishOptions {
        enabled: bool,
        spelling: bool,
    },
    Key {
        value: String,
        #[serde(default)]
        mods: u8,
    },
    Choose {
        index: usize,
        generation: u64,
    },
    Page {
        delta: i32,
    },
    Toggle,
    Reset,
    Literal {
        text: String,
    },
    Ack {
        generation: u64,
        accepted: bool,
    },
}
#[derive(Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Update {
    pub generation: u64,
    pub composition: String,
    pub candidates: Vec<String>,
    pub page_start: usize,
    pub selected: usize,
    pub chinese: bool,
    pub commits: Vec<String>,
    pub pass_through: bool,
}
/// The normal key path returns only the visible window. An unchanged window
/// uses None so Kotlin can retain its strings instead of allocating them again.
#[derive(Debug)]
pub struct CompactUpdate {
    pub generation: u64,
    pub composition: String,
    pub candidates: Option<Vec<String>>,
    pub candidate_start: usize,
    pub candidate_total: usize,
    pub selected: usize,
    pub chinese: bool,
    pub commits: Vec<String>,
    pub pass_through: bool,
}
impl Command {
    pub fn from_input(
        operation: i32,
        text: String,
        argument: i32,
        generation: u64,
        flags: u8,
    ) -> Result<Self, String> {
        match operation {
            1 => Ok(Self::Key {
                value: text,
                mods: flags,
            }),
            2 => Ok(Self::Literal { text }),
            3 if argument >= 0 => Ok(Self::Choose {
                index: argument as usize,
                generation,
            }),
            4 => Ok(Self::Toggle),
            5 => Ok(Self::Reset),
            6 => Ok(Self::EnglishOptions {
                enabled: flags & 1 != 0,
                spelling: flags & 2 != 0,
            }),
            7 => Ok(Self::VoiceLearn { text }),
            _ => Err("invalid input operation".into()),
        }
    }
}
pub struct Session {
    kernel: Kernel,
    render: RenderState,
    last_window: Option<(usize, usize, Vec<String>)>,
    pending: Vec<Event>,
    awaiting: Option<(u64, Vec<Event>)>,
    writer: Option<mpsc::Sender<Vec<Event>>>,
    join: Option<thread::JoinHandle<()>>,
}
impl Session {
    pub fn open(
        dictionary: &Path,
        database: &Path,
        flypy: bool,
        chinese: bool,
        learn: bool,
    ) -> Result<Self, String> {
        Self::open_with_packs(dictionary, database, flypy, chinese, learn, &[])
    }
    pub fn open_with_packs(
        dictionary: &Path,
        database: &Path,
        flypy: bool,
        chinese: bool,
        learn: bool,
        packs: &[String],
    ) -> Result<Self, String> {
        let system: Arc<dyn Lexicon> =
            retype_dict::binary::open_shared(dictionary).map_err(|e| e.to_string())?;
        let mut layered = LayeredDict::new();
        layered.push(Layer {
            name: "base",
            dict: system,
            boost: 0.0,
        });
        for path in packs {
            let dict =
                retype_dict::binary::open_shared(Path::new(path)).map_err(|e| e.to_string())?;
            layered.push(Layer {
                name: "optional",
                dict,
                boost: 0.0,
            });
        }
        Self::with_lexicon(Arc::new(layered), database, flypy, chinese, learn)
    }
    fn with_lexicon(
        system: Arc<dyn Lexicon>,
        database: &Path,
        flypy: bool,
        chinese: bool,
        learn: bool,
    ) -> Result<Self, String> {
        let user = Arc::new(UserDict::new());
        let learner = Arc::new(Learner::with_system(Arc::clone(&user), Arc::clone(&system)));
        let mut writer = None;
        let mut join = None;
        if learn {
            let path = database.to_owned();
            let sys = Arc::clone(&system);
            let cache = Arc::clone(&learner);
            let cache_user = Arc::clone(&user);
            let (tx, rx) = mpsc::channel::<Vec<Event>>();
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            join = Some(thread::spawn(move || {
                let initial_lock = LEARNING_IO.lock().unwrap_or_else(|p| p.into_inner());
                let result = Store::open(&path, sys).map_err(|e| e.to_string());
                let mut store = match result {
                    Ok(store) => store,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let mut request = Request {
                    version: VERSION,
                    client: "android-ime".into(),
                    known_revision: None,
                    events: Vec::new(),
                    stop: false,
                };
                let first = store.handle(&request).map_err(|e| e.to_string());
                let mut sequence = match first {
                    Ok(response) => {
                        if let Some(snapshot) = response.snapshot {
                            snapshot.restore(&cache_user, &cache);
                        }
                        request.known_revision = Some(response.revision);
                        let _ = ready_tx.send(Ok(()));
                        response.acknowledged
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                drop(initial_lock);
                for events in rx {
                    for chunk in events.chunks(retype_learning_store::protocol::MAX_BATCH) {
                        let _lock = LEARNING_IO.lock().unwrap_or_else(|p| p.into_inner());
                        if let Err(e) = store.refresh() {
                            eprintln!("retype Android learning refresh failed: {e}");
                            continue;
                        }
                        request.events = chunk
                            .iter()
                            .cloned()
                            .enumerate()
                            .map(|(i, event)| SequencedEvent {
                                sequence: sequence + i as u64 + 1,
                                event,
                            })
                            .collect();
                        match store.handle(&request) {
                            Ok(response) => {
                                sequence = response.acknowledged;
                                request.known_revision = Some(response.revision);
                                if let Some(snapshot) = response.snapshot {
                                    snapshot.restore(&cache_user, &cache);
                                }
                            }
                            Err(e) => eprintln!("retype Android learning write failed: {e}"),
                        }
                    }
                }
            }));
            ready_rx.recv().map_err(|e| e.to_string())??;
            writer = Some(tx);
        }
        let dict = Arc::new(LayeredDict::with_system_and_user(system, user, 0.0));
        let cfg = KernelConfig {
            rerank_enabled: false,
            chinese_on_start: chinese,
            pinyin_scheme: if flypy {
                PinyinScheme::Flypy
            } else {
                PinyinScheme::Full
            },
            ..Default::default()
        };
        let kernel = Kernel::new(cfg, dict, learner, offline_cloud(Duration::from_millis(1)));
        let render = kernel.render_state();
        Ok(Self {
            kernel,
            render,
            last_window: None,
            pending: Vec::new(),
            awaiting: None,
            writer,
            join,
        })
    }
    pub fn dispatch(&mut self, command: Command) -> Update {
        let (commits, pass) = self.apply(command);
        self.snapshot(commits, pass)
    }
    pub fn dispatch_compact(&mut self, command: Command) -> CompactUpdate {
        let (commits, pass_through) = self.apply(command);
        let state = &self.render;
        let start = state.page_start.min(state.candidates.len());
        let visible = &state.candidates[start..(start + 8).min(state.candidates.len())];
        let unchanged = self
            .last_window
            .as_ref()
            .is_some_and(|(previous_start, total, texts)| {
                *previous_start == start
                    && *total == state.candidates.len()
                    && texts
                        .iter()
                        .map(String::as_str)
                        .eq(visible.iter().map(|c| c.text.as_str()))
            });
        let candidates = if unchanged {
            None
        } else {
            let texts: Vec<_> = visible.iter().map(|c| c.text.clone()).collect();
            self.last_window = Some((start, state.candidates.len(), texts.clone()));
            Some(texts)
        };
        CompactUpdate {
            generation: state.gen,
            composition: state.composition.clone(),
            candidates,
            candidate_start: start,
            candidate_total: state.candidates.len(),
            selected: state.selected,
            chinese: self.kernel.is_chinese(),
            commits,
            pass_through,
        }
    }
    pub fn candidate_page(
        &self,
        generation: u64,
        start: usize,
        limit: usize,
    ) -> Option<Vec<String>> {
        if generation != self.render.gen
            || start > self.render.candidates.len()
            || !(1..=64).contains(&limit)
        {
            return None;
        }
        Some(
            self.render
                .candidates
                .iter()
                .skip(start)
                .take(limit)
                .map(|c| c.text.clone())
                .collect(),
        )
    }
    pub fn acknowledge(&mut self, generation: u64, accepted: bool) {
        self.apply(Command::Ack {
            generation,
            accepted,
        });
    }
    fn apply(&mut self, command: Command) -> (Vec<String>, bool) {
        let mut commits = Vec::new();
        let mut pass = false;
        let event = match command {
            Command::VoiceLearn { text } => {
                // Kotlin calls this only after commitText succeeds on the original editor.
                if let Some(writer) = &self.writer {
                    if !text.is_empty() && text.len() <= 128 * 1024 {
                        let _ = writer.send(vec![Event::Voice { text }]);
                    }
                }
                return (commits, pass);
            }
            Command::EnglishOptions { enabled, spelling } => {
                InputEvent::SetEnglishOptions { enabled, spelling }
            }
            Command::Ack {
                generation,
                accepted,
            } => {
                if self
                    .awaiting
                    .as_ref()
                    .is_some_and(|(g, _)| *g == generation)
                {
                    if let Some((_, events)) = self.awaiting.take() {
                        if accepted {
                            if let Some(writer) = &self.writer {
                                let _ = writer.send(events);
                            }
                        }
                    }
                }
                return (commits, pass);
            }
            Command::Choose { index, generation } => {
                if generation != self.kernel.generation() || index >= self.render.candidates.len() {
                    return (commits, pass);
                }
                InputEvent::CandidateChosen { index }
            }
            Command::Page { delta } => InputEvent::CandidatePage { delta },
            Command::Toggle => InputEvent::ToggleChinese,
            Command::Reset => {
                self.pending.clear();
                self.awaiting = None;
                InputEvent::ResetComposition
            }
            Command::Literal { text } => {
                // Numeric/symbol keys are literal, never candidate selection keys.
                let composition = self.kernel.composition_text();
                if !composition.is_empty() {
                    commits.push(composition);
                }
                if !text.is_empty() {
                    commits.push(text);
                }
                self.pending.clear();
                InputEvent::ResetComposition
            }
            Command::Key { value, mods } => {
                let key = match value.as_str() {
                    "backspace" => Key::Backspace,
                    "enter" => Key::Enter,
                    "space" => Key::Space,
                    "escape" => Key::Escape,
                    "tab" => Key::Tab,
                    "up" => Key::Up,
                    "down" => Key::Down,
                    "left" => Key::Left,
                    "right" => Key::Right,
                    _ => match value.chars().next() {
                        Some(c) => Key::Char(c),
                        None => return (commits, pass),
                    },
                };
                InputEvent::Key {
                    key,
                    mods: Modifiers::from_bits(mods),
                    source: InputSource::Touch,
                }
            }
        };
        let mut queue = std::collections::VecDeque::from([event]);
        while let Some(event) = queue.pop_front() {
            for action in self.kernel.handle(event) {
                match action {
                    KernelAction::Render(state) => self.render = state,
                    KernelAction::Commit(
                        CommitRequest::Text(text) | CommitRequest::ReplaceComposition { text },
                    ) => commits.push(text),
                    KernelAction::PassThrough => pass = true,
                    KernelAction::Side(SideEffect::Learn(event)) => {
                        if let Some(event) = Event::from_learning(event) {
                            self.pending.push(event);
                        }
                    }
                    KernelAction::Side(SideEffect::EnglishSuggest { gen, input }) => queue
                        .push_back(InputEvent::EnglishCompleted {
                            gen,
                            candidates: self.kernel.english_dictionary().correct(&input, 8),
                        }),
                    _ => {}
                }
            }
        }
        if !commits.is_empty() {
            self.awaiting = Some((self.kernel.generation(), std::mem::take(&mut self.pending)));
        }
        (commits, pass)
    }
    fn snapshot(&self, commits: Vec<String>, pass_through: bool) -> Update {
        let state = &self.render;
        Update {
            generation: state.gen,
            composition: state.composition.clone(),
            candidates: state.candidates.iter().map(|c| c.text.clone()).collect(),
            page_start: state.page_start,
            selected: state.selected,
            chinese: self.kernel.is_chinese(),
            commits,
            pass_through,
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.writer.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(target_os = "android")]
mod bridge;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    fn temp() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "retype-android-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[test]
    fn compact_windows_preserve_full_order_stale_guards_and_late_selection() {
        let words: Vec<_> = (0..150).map(|i| format!("候选{i}")).collect();
        let (dict, _) = retype_dict::from_pairs(
            words
                .iter()
                .enumerate()
                .map(|(i, text)| (text.as_str(), "ni", (1000 - i) as f64)),
        );
        let mut s = Session::with_lexicon(Arc::new(dict), &temp(), false, true, false).unwrap();
        s.dispatch_compact(Command::from_input(1, "n".into(), 0, 0, 0).unwrap());
        let first = s.dispatch_compact(Command::from_input(1, "i".into(), 0, 0, 0).unwrap());
        assert!(
            first.candidate_total > 64,
            "only {} candidates",
            first.candidate_total
        );
        assert_eq!(first.candidates.as_ref().unwrap().len(), 8);
        let complete = s.snapshot(Vec::new(), false);
        assert_eq!(
            first.candidates.as_ref().unwrap(),
            &complete.candidates[..8]
        );
        let mut paged = Vec::new();
        for start in (0..first.candidate_total).step_by(64) {
            paged.extend(s.candidate_page(first.generation, start, 64).unwrap());
        }
        assert_eq!(paged, complete.candidates);
        assert!(s.candidate_page(first.generation, 0, 65).is_none());
        assert!(s.candidate_page(first.generation, usize::MAX, 64).is_none());
        let repeated = s.dispatch_compact(Command::Choose {
            index: usize::MAX,
            generation: first.generation,
        });
        assert!(repeated.candidates.is_none());
        s.dispatch_compact(Command::from_input(1, "x".into(), 0, 0, 0).unwrap());
        let second =
            s.dispatch_compact(Command::from_input(1, "backspace".into(), 0, 0, 0).unwrap());
        assert!(s.candidate_page(first.generation, 0, 64).is_none());
        let expected = s
            .candidate_page(second.generation, 64, 1)
            .unwrap()
            .remove(0);
        let stale = s.dispatch_compact(Command::Choose {
            index: 64,
            generation: first.generation,
        });
        assert!(stale.commits.is_empty());
        let selected = s.dispatch_compact(Command::Choose {
            index: 64,
            generation: second.generation,
        });
        assert_eq!(selected.commits, [expected]);
        assert_eq!(selected.candidate_total, 0);
        assert!(Command::from_input(3, String::new(), -1, 0, 0).is_err());
        assert!(Command::from_input(99, String::new(), 0, 0, 0).is_err());
    }
    fn session(path: &Path, flypy: bool, chinese: bool, learn: bool) -> Session {
        let (dict, _) = retype_dict::from_pairs([
            ("你好", "ni hao", 1000.),
            ("我是", "wo shi", 1000.),
            ("额度", "e du", 1000.),
        ]);
        Session::with_lexicon(Arc::new(dict), path, flypy, chinese, learn).unwrap()
    }
    fn type_text(s: &mut Session, text: &str) -> Update {
        let mut out = Update::default();
        for c in text.chars() {
            out = s.dispatch(Command::Key {
                value: c.to_string(),
                mods: 0,
            });
        }
        out
    }
    #[test]
    fn schemes_partial_syllables_and_stale_taps() {
        for (flypy, input, expected) in [
            (false, "nihao", "你好"),
            (true, "nihc", "你好"),
            (true, "edu", "额度"),
        ] {
            let mut s = session(&temp(), flypy, true, false);
            let out = type_text(&mut s, input);
            assert!(out.candidates.iter().any(|c| c == expected));
            assert!(s
                .dispatch(Command::Choose {
                    index: 0,
                    generation: out.generation - 1
                })
                .commits
                .is_empty());
            let at = out.candidates.iter().position(|c| c == expected).unwrap();
            assert_eq!(
                s.dispatch(Command::Choose {
                    index: at,
                    generation: out.generation
                })
                .commits,
                [expected]
            );
        }
    }
    #[test]
    fn english_completion_space_and_literal_numbers() {
        let mut s = session(&temp(), false, false, false);
        assert!(s
            .dispatch(Command::Literal {
                text: String::new()
            })
            .commits
            .is_empty());
        let out = type_text(&mut s, "hel");
        let index = out.candidates.iter().position(|c| c == "hello").unwrap();
        assert_eq!(
            s.dispatch(Command::Choose {
                index,
                generation: out.generation
            })
            .commits,
            ["hello "]
        );
        type_text(&mut s, "python");
        assert_eq!(
            s.dispatch(Command::Literal { text: "3".into() }).commits,
            ["python", "3"]
        );
    }

    #[test]
    fn chinese_modes_select_english_and_restore_personal_learning() {
        for flypy in [false, true] {
            let path = temp();
            let mut s = session(&path, flypy, true, true);
            for _ in 0..2 {
                let out = type_text(&mut s, "Hello");
                let index = out.candidates.iter().position(|c| c == "Hello").unwrap();
                let selected = s.dispatch(Command::Choose {
                    index,
                    generation: out.generation,
                });
                assert_eq!(selected.commits, ["Hello"]);
                s.dispatch(Command::Ack {
                    generation: selected.generation,
                    accepted: true,
                });
            }
            drop(s);
            let mut reopened = session(&path, flypy, true, true);
            let out = type_text(&mut reopened, "hel");
            assert!(out
                .candidates
                .iter()
                .any(|c| c.eq_ignore_ascii_case("hello")));
        }
    }
    #[test]
    fn learns_only_acknowledged_commit_and_survives_reopen() {
        let path = temp();
        {
            let mut s = session(&path, false, false, true);
            type_text(&mut s, "uniqueword");
            let out = s.dispatch(Command::Key {
                value: "space".into(),
                mods: 0,
            });
            s.dispatch(Command::Ack {
                generation: out.generation,
                accepted: false,
            });
            let _ = out;
            for _ in 0..2 {
                type_text(&mut s, "personalword");
                let out = s.dispatch(Command::Key {
                    value: "space".into(),
                    mods: 0,
                });
                assert_eq!(out.commits, ["personalword "]);
                assert!(!s.awaiting.as_ref().unwrap().1.is_empty());
                s.dispatch(Command::Ack {
                    generation: out.generation,
                    accepted: true,
                });
            }
        }
        let mut s = session(&path, false, false, true);
        assert!(type_text(&mut s, "personalwo")
            .candidates
            .iter()
            .any(|c| c == "personalword"));
        s.dispatch(Command::Reset);
        assert!(!type_text(&mut s, "uniquewo")
            .candidates
            .iter()
            .any(|c| c == "uniqueword"));
        drop(s);
        let mut private = session(&path, false, false, false);
        assert!(!type_text(&mut private, "personalwo")
            .candidates
            .iter()
            .any(|c| c == "personalword"));
        drop(private);
        let _ = std::fs::remove_file(path);
    }
}
