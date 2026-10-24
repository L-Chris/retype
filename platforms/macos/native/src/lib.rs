//! InputMethodKit bridge. Loading and persistence run outside the input callback.
//! One runtime shares the dictionary, learner and SQLite writer across clients.
#![allow(unsafe_code)]

mod features;
mod statistics;
#[cfg(test)]
mod tests;

use retype_dict::{LayeredDict, Learner, UserDict};
use retype_engine::{offline_cloud, Kernel, KernelBackend, KernelConfig, LocalBackend};
use retype_learning_store::{
    protocol::{Event, Request, SequencedEvent, VERSION},
    store::Store,
};
use retype_pinyin::Lexicon;
use retype_types::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    ffi::{c_char, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

struct Runtime {
    dictionary: Arc<dyn Lexicon>,
    system: Arc<retype_dict::AsyncDict>,
    learner: Arc<Learner>,
    writer: mpsc::Sender<LearningJob>,
    statistics: statistics::Writer,
}
enum LearningJob {
    Events(Vec<Event>),
    Flush(mpsc::Sender<()>),
}
impl Runtime {
    fn open(root: &Path) -> Result<Arc<Self>, String> {
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let status = retype_resources::activate(&root.join("dictionaries"))?;
        let dictionary = status.base.ok_or("请先在设置中安装基础词库")?;
        let dictionary =
            retype_dict::binary::open_shared(&dictionary).map_err(|e| e.to_string())?;
        Self::with_dictionary(dictionary, &root.join("learning.db"))
    }
    fn with_dictionary(dictionary: Arc<dyn Lexicon>, db: &Path) -> Result<Arc<Self>, String> {
        let guard = retype_services::LEARNING_IO
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let system = retype_dict::AsyncDict::with(dictionary);
        let dictionary: Arc<dyn Lexicon> = Arc::clone(&system) as Arc<dyn Lexicon>;
        let user = Arc::new(UserDict::new());
        let learner = Arc::new(Learner::with_system(
            Arc::clone(&user),
            Arc::clone(&dictionary),
        ));
        let mut store = Store::open(db, Arc::clone(&dictionary)).map_err(|e| e.to_string())?;
        let mut request = Request {
            version: VERSION,
            client: "macos-ime".into(),
            known_revision: None,
            events: Vec::new(),
            stop: false,
        };
        let response = store.handle(&request).map_err(|e| e.to_string())?;
        if let Some(snapshot) = response.snapshot {
            snapshot.restore(&user, &learner);
        }
        let mut sequence = response.acknowledged;
        request.known_revision = Some(response.revision);
        drop(guard);
        let (writer, rx) = mpsc::channel::<LearningJob>();
        let cache = Arc::clone(&learner);
        let cache_user = Arc::clone(&user);
        std::thread::Builder::new()
            .name("retype-mac-learning".into())
            .spawn(move || {
                for job in rx {
                    let events = match job {
                        LearningJob::Events(events) => events,
                        LearningJob::Flush(done) => {
                            let _ = done.send(());
                            continue;
                        }
                    };
                    let _guard = retype_services::LEARNING_IO
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    if let Err(e) = store.refresh() {
                        eprintln!("retype macOS learning refresh failed: {e}");
                        continue;
                    }
                    request.events = events
                        .into_iter()
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
                        Err(e) => eprintln!("retype macOS learning write failed: {e}"),
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let dictionary = Arc::new(LayeredDict::with_system_and_user(dictionary, user, 0.0));
        Ok(Arc::new(Self {
            dictionary,
            system,
            learner,
            writer,
            statistics: statistics::Writer::new(db.with_extension("statistics"))?,
        }))
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Command {
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
    Layout {
        generation: u64,
        widths: Vec<i32>,
        available: i32,
    },
    Configure {
        flypy: bool,
        english: bool,
        spelling: bool,
    },
    Toggle,
    Reset,
    Discard,
    Poll,
    Ack {
        token: u64,
        accepted: bool,
    },
    Boundary,
}
struct PendingCommit {
    token: u64,
    events: Vec<Event>,
    text: Vec<String>,
    input: retype_types::statistics::InputStats,
}
struct Session {
    runtime: Arc<Runtime>,
    backend: Arc<LocalBackend>,
    pending: Vec<Event>,
    awaiting: Option<PendingCommit>,
    commit_token: u64,
    activity: statistics::Activity,
}
impl Session {
    fn new(runtime: Arc<Runtime>, flypy: bool, chinese: bool) -> Self {
        let cloud = offline_cloud(Duration::from_millis(1));
        let kernel = Kernel::new(
            KernelConfig {
                decode: retype_pinyin::DecodeOptions {
                    page_size: 8,
                    ..Default::default()
                },
                rerank_enabled: false,
                chinese_on_start: chinese,
                pinyin_scheme: if flypy {
                    PinyinScheme::Flypy
                } else {
                    PinyinScheme::Full
                },
                ..Default::default()
            },
            Arc::clone(&runtime.dictionary),
            Arc::clone(&runtime.learner) as Arc<dyn LearningStore>,
            Arc::clone(&cloud),
        );
        Self {
            runtime,
            backend: LocalBackend::new(kernel, cloud),
            pending: Vec::new(),
            awaiting: None,
            commit_token: 0,
            activity: Default::default(),
        }
    }
    fn dispatch(&mut self, command: Command) -> Result<Value, String> {
        let mut actions = Vec::new();
        let before = self
            .backend
            .with_kernel(|k| (k.has_composition(), k.spelling_len()));
        let observes = !matches!(
            &command,
            Command::Ack { .. }
                | Command::Poll
                | Command::Boundary
                | Command::Layout { .. }
                | Command::Discard
        );
        let backspace =
            matches!(&command,Command::Key{value,mods} if value=="backspace"&&mods & !1==0);
        let literal = match &command {
            Command::Key { value, mods } if mods & !1 == 0 => {
                self.activity.tick();
                match value.as_str() {
                    "space" => " ".into(),
                    "enter" | "tab" => "\n".into(),
                    "backspace" => String::new(),
                    _ if value.chars().count() == 1 => value.clone(),
                    _ => String::new(),
                }
            }
            _ => String::new(),
        };
        let event = match command {
            Command::Key { value, mods } => {
                let key = match value.as_str() {
                    "backspace" => Key::Backspace,
                    "delete" => Key::Delete,
                    "enter" => Key::Enter,
                    "space" => Key::Space,
                    "escape" => Key::Escape,
                    "tab" => Key::Tab,
                    "up" => Key::Up,
                    "down" => Key::Down,
                    "left" => Key::Left,
                    "right" => Key::Right,
                    "home" => Key::Home,
                    "end" => Key::End,
                    "pageUp" => Key::PageUp,
                    "pageDown" => Key::PageDown,
                    _ => {
                        let mut chars = value.chars();
                        let c = chars.next().ok_or("empty key")?;
                        if chars.next().is_some() {
                            return Err("key must contain one character".into());
                        }
                        Key::Char(c)
                    }
                };
                Some(InputEvent::Key {
                    key,
                    mods: Modifiers::from_bits(mods),
                    source: InputSource::Keyboard,
                })
            }
            Command::Choose { index, generation } => {
                let state = self.backend.render();
                (state.gen == generation && index < state.candidates.len())
                    .then_some(InputEvent::CandidateChosen { index })
            }
            Command::Page { delta } => Some(InputEvent::CandidatePage {
                delta: delta.clamp(-1, 1),
            }),
            Command::Toggle => Some(InputEvent::ToggleChinese),
            Command::Discard => {
                self.activity.input.discard();
                self.pending.clear();
                self.awaiting = None;
                Some(InputEvent::ResetComposition)
            }
            Command::Reset => {
                self.pending.clear();
                self.awaiting = None;
                Some(InputEvent::ResetComposition)
            }
            Command::Boundary => {
                let counts = self.activity.boundary();
                self.runtime.statistics.record(counts);
                None
            }
            Command::Configure {
                flypy,
                english,
                spelling,
            } => {
                actions.extend(self.backend.submit_deferred_learning(
                    InputEvent::SetPinyinScheme(if flypy {
                        PinyinScheme::Flypy
                    } else {
                        PinyinScheme::Full
                    }),
                    false,
                ));
                Some(InputEvent::SetEnglishOptions {
                    enabled: english,
                    spelling,
                })
            }
            Command::Layout {
                generation,
                widths,
                available,
            } => {
                let state = self.backend.render();
                if generation == state.gen
                    && widths.len() == state.candidates.len()
                    && (80..=480).contains(&available)
                    && widths.iter().all(|w| (1..=10000).contains(w))
                {
                    self.backend.layout_candidates(&widths, available, 0);
                }
                None
            }
            Command::Ack { token, accepted } => {
                if self.awaiting.as_ref().is_some_and(|p| p.token == token) {
                    if let Some(pending) = self.awaiting.take() {
                        if accepted {
                            let _ = self
                                .runtime
                                .writer
                                .send(LearningJob::Events(pending.events));
                            let mut counts = self.activity.commit_counts(&pending.text.concat());
                            counts.input = pending.input;
                            self.runtime.statistics.record(counts);
                        }
                    }
                }
                None
            }
            Command::Poll => None,
        };
        if let Some(event) = event {
            actions.extend(self.backend.submit_deferred_learning(event, true));
        }
        while let Some(action) = self.backend.poll_action() {
            actions.push(action);
        }
        let mut commits = Vec::new();
        let mut pass = false;
        for action in actions {
            match action {
                KernelAction::Commit(
                    CommitRequest::Text(t) | CommitRequest::ReplaceComposition { text: t },
                ) => commits.push(t),
                KernelAction::Side(SideEffect::Learn(e)) => {
                    if let Some(e) = Event::from_learning(e) {
                        self.pending.push(e);
                    }
                }
                KernelAction::PassThrough => pass = true,
                _ => {}
            }
        }
        let after = self
            .backend
            .with_kernel(|k| (k.has_composition(), k.spelling_len()));
        if observes {
            self.activity.input.edit(before, after, backspace);
        }
        if !commits.is_empty() {
            self.commit_token += 1;
            self.awaiting = Some(PendingCommit {
                token: self.commit_token,
                events: std::mem::take(&mut self.pending),
                input: self.activity.input.commit(&commits.concat()),
                text: commits.clone(),
            });
        } else if observes && before.0 && !after.0 {
            let counts = self.activity.boundary();
            self.runtime.statistics.record(counts);
        } else if pass && backspace {
            let counts = self.activity.backspace();
            self.runtime.statistics.record(counts);
        } else if pass && !literal.is_empty() {
            self.runtime
                .statistics
                .record(self.activity.commit(&literal));
        }
        let state = self.backend.render();
        Ok(
            json!({ "generation": state.gen, "composition": state.composition,
            "candidates": state.candidates.iter().map(|c| &c.text).collect::<Vec<_>>(),
            "pageStart": state.page_start, "pageSize": state.page_size, "selected": state.selected,
            "chinese": state.status.contains(StatusFlags::CHINESE),
            "commits": commits, "passThrough": pass, "token": self.commit_token }),
        )
    }
}

pub struct RtRuntime(Arc<Runtime>);
pub struct RtSession(Session);
pub struct RtResources {
    root: std::path::PathBuf,
    cancelled: AtomicBool,
    progress: AtomicU32,
}

fn output(value: Value) -> *mut c_char {
    CString::new(value.to_string()).map_or(std::ptr::null_mut(), CString::into_raw)
}
fn boundary(f: impl FnOnce() -> Result<Value, String>) -> *mut c_char {
    output(match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => json!({"error": e}),
        Err(_) => json!({"error":"输入内核发生错误"}),
    })
}
unsafe fn text<'a>(p: *const c_char) -> Result<&'a str, String> {
    if p.is_null() {
        return Err("null string".into());
    }
    CStr::from_ptr(p).to_str().map_err(|e| e.to_string())
}

/// # Safety
/// Strings must be valid NUL-terminated UTF-8; handles must be live and used on one serial queue.
/// Runtime/resource opening and resource operations belong on a background queue.
#[no_mangle]
pub unsafe extern "C" fn rt_runtime_open(
    root: *const c_char,
    out: *mut *mut RtRuntime,
) -> *mut c_char {
    boundary(|| {
        if out.is_null() {
            return Err("null output".into());
        }
        *out = std::ptr::null_mut();
        let runtime = Runtime::open(Path::new(text(root)?))?;
        *out = Box::into_raw(Box::new(RtRuntime(runtime)));
        Ok(json!({"ready":true}))
    })
}
/// # Safety
/// Handle is null or a live runtime. Sessions retain their own runtime reference.
#[no_mangle]
pub unsafe extern "C" fn rt_runtime_free(p: *mut RtRuntime) {
    if !p.is_null() {
        drop(Box::from_raw(p));
    }
}
/// # Safety
/// Runtime must be live. The returned session belongs to the caller.
#[no_mangle]
pub unsafe extern "C" fn rt_session_new(
    p: *const RtRuntime,
    flypy: bool,
    chinese: bool,
) -> *mut RtSession {
    catch_unwind(AssertUnwindSafe(|| {
        p.as_ref().map(|r| {
            Box::into_raw(Box::new(RtSession(Session::new(
                Arc::clone(&r.0),
                flypy,
                chinese,
            ))))
        })
    }))
    .ok()
    .flatten()
    .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// Handle is null or a live session, with no concurrent calls.
#[no_mangle]
pub unsafe extern "C" fn rt_session_free(p: *mut RtSession) {
    if !p.is_null() {
        drop(Box::from_raw(p));
    }
}
/// # Safety
/// Session must be live and command a NUL-terminated UTF-8 JSON string. Free the result with rt_string_free.
#[no_mangle]
pub unsafe extern "C" fn rt_session_send(p: *mut RtSession, command: *const c_char) -> *mut c_char {
    boundary(|| {
        let command = text(command)?;
        if command.len() > 64 * 1024 {
            return Err("command too large".into());
        }
        let command = serde_json::from_str(command).map_err(|e| e.to_string())?;
        p.as_mut().ok_or("null session")?.0.dispatch(command)
    })
}
/// # Safety
/// Command is a live NUL-terminated UTF-8 JSON string. Run on a worker and free the result.
#[no_mangle]
pub unsafe extern "C" fn rt_feature(command: *const c_char) -> *mut c_char {
    boundary(|| {
        let text = text(command)?;
        if text.len() > 4 * 1024 * 1024 {
            return Err("请求过大".into());
        }
        let request = serde_json::from_str(text).map_err(|_| "请求格式无效")?;
        features::perform(request).map(|v| json!({"value":v}))
    })
}
/// # Safety
/// Runtime and root must remain live throughout this worker call.
#[no_mangle]
pub unsafe extern "C" fn rt_runtime_reload(
    runtime: *const RtRuntime,
    root: *const c_char,
    pack_mask: u32,
) -> *mut c_char {
    boundary(|| {
        let runtime = runtime.as_ref().ok_or("输入内核未就绪")?;
        runtime.0.system.install(features::dictionary(
            Path::new(text(root)?),
            pack_mask & 127,
        )?);
        let _ = runtime.0.writer.send(LearningJob::Events(Vec::new()));
        Ok(json!({"ready":true}))
    })
}
/// # Safety
/// Runtime and UTF-8 text must be live; call only after the editor accepted voice output.
#[no_mangle]
pub unsafe extern "C" fn rt_runtime_voice(
    runtime: *const RtRuntime,
    value: *const c_char,
) -> *mut c_char {
    boundary(|| {
        let runtime = runtime.as_ref().ok_or("输入内核未就绪")?;
        let text = text(value)?;
        if text.len() > 128 * 1024 {
            return Err("语音文本过长".into());
        }
        let _ = runtime
            .0
            .writer
            .send(LearningJob::Events(vec![Event::Voice {
                text: text.into(),
            }]));
        runtime.0.statistics.voice(text);
        Ok(json!(true))
    })
}
/// # Safety
/// Runtime must stay live while this worker call drains acknowledged learning and numeric statistics.
#[no_mangle]
pub unsafe extern "C" fn rt_runtime_flush(runtime: *const RtRuntime) -> *mut c_char {
    boundary(|| {
        let runtime = runtime.as_ref().ok_or("输入内核未就绪")?;
        let (done, rx) = mpsc::channel();
        runtime
            .0
            .writer
            .send(LearningJob::Flush(done))
            .map_err(|_| "学习后台已退出")?;
        rx.recv_timeout(Duration::from_secs(2))
            .map_err(|_| "保存学习记录超时")?;
        runtime.0.statistics.flush()?;
        Ok(json!(true))
    })
}
/// # Safety
/// Callbacks are process-lived, thread-safe and honor the buffer lengths. Clipboard callbacks marshal to the main queue.
#[no_mangle]
pub unsafe extern "C" fn rt_macos_hooks(
    read: unsafe extern "C" fn(*const c_char, *mut u8, usize) -> isize,
    save: unsafe extern "C" fn(*const c_char, *const c_char) -> bool,
    clipboard_read: unsafe extern "C" fn(*mut u8, usize, *mut u64) -> isize,
    clipboard_write: unsafe extern "C" fn(*const c_char, u64, *mut u64) -> bool,
) {
    #[cfg(target_os = "macos")]
    {
        retype_ai::secrets::register(read, save);
        retype_sync::lan::macos::register(clipboard_read, clipboard_write);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (read, save, clipboard_read, clipboard_write);
}
/// # Safety
/// Pointer is null or an owned string returned by this library, freed once.
#[no_mangle]
pub unsafe extern "C" fn rt_string_free(p: *mut c_char) {
    if !p.is_null() {
        drop(CString::from_raw(p));
    }
}
/// # Safety
/// Root must be a valid UTF-8 C string. Caller owns returned handle.
#[no_mangle]
pub unsafe extern "C" fn rt_resources_new(root: *const c_char) -> *mut RtResources {
    catch_unwind(AssertUnwindSafe(|| {
        text(root).map(|root| {
            Box::into_raw(Box::new(RtResources {
                root: Path::new(root).join("dictionaries"),
                cancelled: AtomicBool::new(false),
                progress: AtomicU32::new(0),
            }))
        })
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// Resource handle must remain live until the operation returns; only one operation at a time.
/// Action: 0=status, 1=download, 2=import. Import source is a valid C string for action 2.
#[no_mangle]
pub unsafe extern "C" fn rt_resources_run(
    p: *const RtResources,
    action: i32,
    source: *const c_char,
) -> *mut c_char {
    boundary(|| {
        let r = p.as_ref().ok_or("null resources")?;
        let status = match action {
            0 => retype_resources::status(&r.root),
            1 => retype_resources::download(&r.root, &r.cancelled, |v| {
                r.progress.store((v * 1000.) as u32, Ordering::Relaxed)
            }),
            2 => retype_resources::import_bundle(&r.root, Path::new(text(source)?), &r.cancelled),
            _ => Err("invalid resource action".into()),
        }?;
        if action != 0 {
            retype_resources::activate(&r.root)?;
        }
        serde_json::to_value(status).map_err(|e| e.to_string())
    })
}
/// # Safety
/// Resource handle must be live with no operation in flight. Call before scheduling a new operation.
#[no_mangle]
pub unsafe extern "C" fn rt_resources_begin(p: *const RtResources) {
    if let Some(r) = p.as_ref() {
        r.cancelled.store(false, Ordering::Relaxed);
        r.progress.store(0, Ordering::Relaxed);
    }
}
/// # Safety
/// Resource handle must be live. Safe concurrently with rt_resources_run.
#[no_mangle]
pub unsafe extern "C" fn rt_resources_cancel(p: *const RtResources) {
    if let Some(r) = p.as_ref() {
        r.cancelled.store(true, Ordering::Relaxed);
    }
}
/// # Safety
/// Resource handle must be live. Safe concurrently with rt_resources_run.
#[no_mangle]
pub unsafe extern "C" fn rt_resources_progress(p: *const RtResources) -> u32 {
    p.as_ref().map_or(0, |r| r.progress.load(Ordering::Relaxed))
}
/// # Safety
/// No operation or progress call may be in flight when freeing the resource handle.
#[no_mangle]
pub unsafe extern "C" fn rt_resources_free(p: *mut RtResources) {
    if !p.is_null() {
        drop(Box::from_raw(p));
    }
}
