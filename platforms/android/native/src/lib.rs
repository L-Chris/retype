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
pub struct Session {
    kernel: Kernel,
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
        Ok(Self {
            kernel: Kernel::new(cfg, dict, learner, offline_cloud(Duration::from_millis(1))),
            pending: Vec::new(),
            awaiting: None,
            writer,
            join,
        })
    }
    pub fn dispatch(&mut self, command: Command) -> Update {
        let mut commits = Vec::new();
        let mut pass = false;
        let event = match command {
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
                return self.snapshot(commits, pass);
            }
            Command::Choose { index, generation } => {
                if generation != self.kernel.generation()
                    || index >= self.kernel.render_state().candidates.len()
                {
                    return self.snapshot(commits, pass);
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
                        None => return self.snapshot(commits, pass),
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
                            candidates: retype_english::correct(&input, 8),
                        }),
                    _ => {}
                }
            }
        }
        if !commits.is_empty() {
            self.awaiting = Some((self.kernel.generation(), std::mem::take(&mut self.pending)));
        }
        self.snapshot(commits, pass)
    }
    fn snapshot(&self, commits: Vec<String>, pass_through: bool) -> Update {
        let state = self.kernel.render_state();
        Update {
            generation: state.gen,
            composition: state.composition,
            candidates: state.candidates.into_iter().map(|c| c.text).collect(),
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
