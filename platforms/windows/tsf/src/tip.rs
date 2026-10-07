//! Windows TSF lifecycle and keyboard routing. Text changes run under TSF edit locks.
use crate::{candidate::CandidateWindow, display, edit, keymap, session::Session, stats};

use retype_types::{InputEvent, Key, Modifiers};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use windows::Win32::Foundation::{E_INVALIDARG, E_POINTER, E_UNEXPECTED, LPARAM, WPARAM};
use windows::Win32::UI::TextServices::*;
use windows_core::{implement, Interface, Ref, Result, BOOL, GUID};

const VOICE_COMMAND: GUID = GUID::from_u128(0x813e19d9_51f0_4b3f_9d7a_a21ef2e1d652);
const TRANSLATION_COMMAND: GUID = GUID::from_u128(0x40926c8b_8ca6_4ae0_b17c_706ac447a38d);

fn preserved_voice(binding: retype_ai::config::Shortcut) -> Option<TF_PRESERVEDKEY> {
    if binding == retype_ai::config::Shortcut::VOICE {
        Some(TF_PRESERVEDKEY {
            uVKey: 0x12,
            uModifiers: TF_MOD_RALT,
        })
    } else {
        preserved_translation(binding)
    }
}
fn right_alt_event(vk: WPARAM, lp: LPARAM) -> bool {
    vk.0 == 0xa5 || (vk.0 == 0x12 && lp.0 & (1 << 24) != 0)
}
fn voice_matches(binding: retype_ai::config::Shortcut, vk: WPARAM, lp: LPARAM, bits: u8) -> bool {
    if binding == retype_ai::config::Shortcut::VOICE {
        // Exclude left Alt and AltGr/Ctrl+Alt shortcuts.
        right_alt_event(vk, lp) && bits == 2
    } else {
        binding.vk != 0 && binding.vk == (vk.0 & 0xffff) as u16 && binding.modifiers == bits
    }
}

fn preserved_translation(binding: retype_ai::config::Shortcut) -> Option<TF_PRESERVEDKEY> {
    // TSF's preserved-key API has no Windows-key modifier. Keep those bindings
    // on the ordinary key-event path rather than registering a different chord.
    if binding.vk == 0 || binding.modifiers & 8 != 0 || !binding.valid(false) {
        return None;
    }
    Some(TF_PRESERVEDKEY {
        uVKey: u32::from(binding.vk),
        uModifiers: if binding.modifiers & 1 != 0 {
            TF_MOD_CONTROL
        } else {
            0
        } | if binding.modifiers & 2 != 0 {
            TF_MOD_ALT
        } else {
            0
        } | if binding.modifiers & 4 != 0 {
            TF_MOD_SHIFT
        } else {
            0
        },
    })
}

fn normalized_modifier(vk: WPARAM) -> u16 {
    match vk.0 as u16 {
        0xa0 | 0xa1 => 0x10,
        0xa2 | 0xa3 => 0x11,
        value => value,
    }
}
fn settings_host() -> bool {
    static SETTINGS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SETTINGS.get_or_init(|| {
        std::env::current_exe().ok().is_some_and(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            name.eq_ignore_ascii_case("retype-settings-egui.exe")
                || (name.eq_ignore_ascii_case("retype.exe")
                    && path
                        .parent()
                        .and_then(|p| p.file_name())
                        .is_some_and(|p| p.eq_ignore_ascii_case("settings")))
        })
    })
}
fn shortcut_modifiers(mods: Modifiers) -> u8 {
    u8::from(mods.contains(Modifiers::CTRL))
        | (u8::from(mods.contains(Modifiers::ALT)) * 2)
        | (u8::from(mods.contains(Modifiers::SHIFT)) * 4)
        | (u8::from(mods.contains(Modifiers::WIN)) * 8)
}
fn tap_modifiers(vk: u16) -> Modifiers {
    let bits = shortcut_modifiers(keymap::read_modifiers());
    let allowed = if vk == 0x10 { 4 } else { 1 };
    if bits & !allowed == 0 {
        Modifiers::NONE
    } else {
        Modifiers::CTRL
    }
}

#[derive(Default)]
struct ShiftTap {
    pressed: bool,
    used_with_other_key: bool,
}
impl ShiftTap {
    fn press(&mut self, mods: Modifiers) {
        if self.pressed {
            self.used_with_other_key = true;
        } else {
            self.pressed = true;
            self.used_with_other_key = !mods.is_plain();
        }
    }
    fn other_key(&mut self) {
        if self.pressed {
            self.used_with_other_key = true;
        }
    }
    fn release(&mut self, mods: Modifiers) -> bool {
        let toggle = self.pressed && !self.used_with_other_key && mods.is_plain();
        *self = Self::default();
        toggle
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
pub(crate) fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(E_UNEXPECTED.into()))
}

pub struct TipState {
    pub tid: AtomicU32,
    pub activated: AtomicBool,
    pub thread_mgr: Mutex<Option<ITfThreadMgr>>,
    pub session: Mutex<Option<Arc<Session>>>,
    pub(crate) composition: Mutex<Option<edit::Composition>>,
    pub(crate) pending: AtomicU32,
    pub(crate) epoch: AtomicU32,
    pub(crate) writing: AtomicBool,
    pub(crate) attribute: AtomicU32,
    focus_cookie: Mutex<Vec<u32>>,
    pub(crate) window: Mutex<Option<CandidateWindow>>,
    pub(crate) translation_window: Mutex<Option<usize>>,
    pub(crate) voice_window: Mutex<Option<usize>>,
    voice_alt_down: AtomicBool,
    language_bar: Mutex<Option<crate::langbar::LanguageBar>>,
    shift_tap: Mutex<ShiftTap>,
    shortcuts: Mutex<(std::time::Instant, retype_ai::config::Shortcuts)>,
    preserved_translation: Mutex<Option<retype_ai::config::Shortcut>>,
    preserved_voice: Mutex<Option<retype_ai::config::Shortcut>>,
    mode_bridge: Mutex<Option<crate::langbar::ModeBridge>>,
    pub(crate) stats_clock: Mutex<stats::ActivityClock>,
}
impl TipState {
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            tid: AtomicU32::new(0),
            activated: AtomicBool::new(false),
            thread_mgr: Mutex::new(None),
            session: Mutex::new(None),
            composition: Mutex::new(None),
            pending: AtomicU32::new(0),
            epoch: AtomicU32::new(0),
            writing: AtomicBool::new(false),
            attribute: AtomicU32::new(0),
            focus_cookie: Mutex::new(Vec::new()),
            window: Mutex::new(Some(CandidateWindow::default())),
            translation_window: Mutex::new(None),
            voice_window: Mutex::new(None),
            voice_alt_down: AtomicBool::new(false),
            language_bar: Mutex::new(None),
            shift_tap: Mutex::new(ShiftTap::default()),
            shortcuts: Mutex::new((std::time::Instant::now(), retype_ai::secrets::shortcuts())),
            preserved_translation: Mutex::new(None),
            preserved_voice: Mutex::new(None),
            mode_bridge: Mutex::new(None),
            stats_clock: Mutex::new(stats::ActivityClock::default()),
        })
    }
    pub fn activate(
        self: &Arc<Self>,
        ptim: Ref<'_, ITfThreadMgr>,
        tid: u32,
        _flags: u32,
    ) -> Result<()> {
        let mgr = ptim.ok()?.clone();
        self.tid.store(tid, Ordering::SeqCst);
        *lock(&self.thread_mgr) = Some(mgr.clone());
        *lock(&self.session) = Some(Session::start());
        // SAFETY: All COM calls run in the calling TSF apartment with valid interfaces.
        unsafe {
            let categories: ITfCategoryMgr = windows::Win32::System::Com::CoCreateInstance(
                &CLSID_TF_CategoryMgr,
                None,
                windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
            )?;
            self.attribute.store(
                categories.RegisterGUID(&display::ATTRIBUTE)?,
                Ordering::SeqCst,
            );
            let keys: ITfKeystrokeMgr = mgr.cast()?;
            let sink: ITfKeyEventSink = KeyEventSink {
                state: Arc::downgrade(self),
            }
            .into();
            keys.AdviseKeyEventSink(tid, &sink, true)?;
            let source: ITfSource = mgr.cast()?;
            let focus: ITfThreadFocusSink = FocusSink {
                state: Arc::downgrade(self),
            }
            .into();
            match source.AdviseSink(&ITfThreadFocusSink::IID, &focus) {
                Ok(cookie) => lock(&self.focus_cookie).push(cookie),
                Err(e) => {
                    let _ = keys.UnadviseKeyEventSink(tid);
                    return Err(e);
                }
            }
            let documents: ITfThreadMgrEventSink = focus.cast()?;
            match source.AdviseSink(&ITfThreadMgrEventSink::IID, &documents) {
                Ok(cookie) => lock(&self.focus_cookie).push(cookie),
                Err(error) => {
                    for cookie in lock(&self.focus_cookie).drain(..) {
                        let _ = source.UnadviseSink(cookie);
                    }
                    let _ = keys.UnadviseKeyEventSink(tid);
                    return Err(error);
                }
            }
        }
        self.activated.store(true, Ordering::SeqCst);
        let translation_binding = lock(&self.shortcuts).1.translate;
        self.sync_translation_key(translation_binding);
        let voice_binding = lock(&self.shortcuts).1.voice;
        self.sync_voice_key(voice_binding);
        if let Ok(bridge) = crate::langbar::ModeBridge::attach(self, &mgr) {
            *lock(&self.mode_bridge) = Some(bridge);
        }
        match crate::langbar::LanguageBar::attach(self, &mgr) {
            Ok(bar) => *lock(&self.language_bar) = Some(bar),
            Err(error) => {
                // Some packaged hosts do not expose a language-bar manager. The
                // key sink and edit sessions remain usable without a mode icon.
                tracing::warn!("TSF language bar is unavailable: {error}");
            }
        }
        Ok(())
    }
    pub fn deactivate(self: &Arc<Self>) -> Result<()> {
        crate::translation::dismiss(self);
        crate::voice::dismiss(self);
        self.voice_alt_down.store(false, Ordering::Release);
        self.activated.store(false, Ordering::SeqCst);
        *lock(&self.shift_tap) = ShiftTap::default();
        lock(&self.stats_clock).reset();
        let bridge = lock(&self.mode_bridge).take();
        drop(bridge);
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.hide();
        self.finish(false);
        let bar = lock(&self.language_bar).take();
        drop(bar);
        let mgr = lock(&self.thread_mgr).take();
        if let Some(mgr) = mgr {
            // SAFETY: Registered sinks belong to this manager and client id.
            unsafe {
                if let Ok(keys) = mgr.cast::<ITfKeystrokeMgr>() {
                    let voice_binding = lock(&self.preserved_voice).take();
                    if let Some(key) = voice_binding.and_then(preserved_voice) {
                        let _ = keys.UnpreserveKey(&VOICE_COMMAND, &key);
                    }
                    let translation_binding = lock(&self.preserved_translation).take();
                    if let Some(binding) = translation_binding {
                        if let Some(key) = preserved_translation(binding) {
                            let _ = keys.UnpreserveKey(&TRANSLATION_COMMAND, &key);
                        }
                    }
                    let _ = keys.UnadviseKeyEventSink(self.tid.load(Ordering::SeqCst));
                }
                let cookies = std::mem::take(&mut *lock(&self.focus_cookie));
                if let Ok(source) = mgr.cast::<ITfSource>() {
                    for cookie in cookies {
                        let _ = source.UnadviseSink(cookie);
                    }
                }
            }
        }
        lock(&self.session).take();
        Ok(())
    }
    pub fn is_activated(&self) -> bool {
        self.activated.load(Ordering::SeqCst)
    }
    pub(crate) fn shortcuts(&self) -> retype_ai::config::Shortcuts {
        let (bindings, refresh) = {
            let mut cache = lock(&self.shortcuts);
            let refresh = cache.0.elapsed() >= std::time::Duration::from_millis(250);
            if refresh {
                *cache = (std::time::Instant::now(), retype_ai::secrets::shortcuts());
            }
            (cache.1, refresh)
        };
        if refresh && self.is_activated() {
            self.sync_translation_key(bindings.translate);
            self.sync_voice_key(bindings.voice);
        }
        bindings
    }
    fn sync_translation_key(&self, binding: retype_ai::config::Shortcut) {
        let desired = preserved_translation(binding).map(|_| binding);
        let previous = *lock(&self.preserved_translation);
        if previous == desired || settings_host() {
            return;
        }
        let manager = lock(&self.thread_mgr).clone();
        let Some(keys) = manager.and_then(|mgr| mgr.cast::<ITfKeystrokeMgr>().ok()) else {
            return;
        };
        // SAFETY: Registration belongs to this active TSF client and apartment.
        // Never hold our mutexes across calls that can invoke a TSF callback.
        let result = unsafe {
            if let Some(previous) = previous.and_then(preserved_translation) {
                if let Err(error) = keys.UnpreserveKey(&TRANSLATION_COMMAND, &previous) {
                    crate::settings_log::event(
                        "launcher",
                        "translation_key_unregister_failed",
                        0,
                        format!("hresult={}", error.code().0),
                    );
                    return;
                }
            }
            *lock(&self.preserved_translation) = None;
            if let Some(key) = preserved_translation(binding) {
                keys.PreserveKey(
                    self.tid.load(Ordering::SeqCst),
                    &TRANSLATION_COMMAND,
                    &key,
                    &"retype translation".encode_utf16().collect::<Vec<_>>(),
                )
            } else {
                Ok(())
            }
        };
        if result.is_ok() {
            *lock(&self.preserved_translation) = desired;
        }
        crate::settings_log::event(
            "launcher",
            "translation_key_registration",
            0,
            format!(
                "enabled={} success={} hresult={}",
                desired.is_some(),
                result.is_ok(),
                result.as_ref().err().map_or(0, |e| e.code().0)
            ),
        );
    }
    fn sync_voice_key(&self, binding: retype_ai::config::Shortcut) {
        let desired = preserved_voice(binding).map(|_| binding);
        let previous = *lock(&self.preserved_voice);
        if previous == desired || settings_host() {
            return;
        }
        let manager = lock(&self.thread_mgr).clone();
        let Some(keys) = manager.and_then(|mgr| mgr.cast::<ITfKeystrokeMgr>().ok()) else {
            return;
        };
        // SAFETY: Registration belongs to this active TSF client and apartment.
        // Never hold our mutexes across calls that can invoke a TSF callback.
        let result = unsafe {
            if let Some(previous) = previous.and_then(preserved_voice) {
                if let Err(error) = keys.UnpreserveKey(&VOICE_COMMAND, &previous) {
                    crate::settings_log::event(
                        "launcher",
                        "voice_key_unregister_failed",
                        0,
                        format!("hresult={}", error.code().0),
                    );
                    return;
                }
            }
            *lock(&self.preserved_voice) = None;
            if let Some(key) = preserved_voice(binding) {
                keys.PreserveKey(
                    self.tid.load(Ordering::SeqCst),
                    &VOICE_COMMAND,
                    &key,
                    &"retype voice".encode_utf16().collect::<Vec<_>>(),
                )
            } else {
                Ok(())
            }
        };
        if result.is_ok() {
            *lock(&self.preserved_voice) = desired;
        }
        crate::settings_log::event(
            "launcher",
            "voice_key_registration",
            0,
            format!(
                "enabled={} success={} hresult={}",
                desired.is_some(),
                result.is_ok(),
                result.as_ref().err().map_or(0, |e| e.code().0)
            ),
        );
    }
    pub fn session(&self) -> Option<Arc<Session>> {
        lock(&self.session).clone()
    }
    pub(crate) fn notify_language_bar(&self) {
        let compartment = lock(&self.mode_bridge)
            .as_ref()
            .map(|bridge| bridge.compartment.clone());
        if let (Some(compartment), Some(session)) = (compartment, self.session()) {
            let value = i32::from(session.backend.with_kernel(|k| k.is_chinese()));
            // SAFETY: Publish OS IME state outside locks; the callback compares before applying.
            unsafe {
                if compartment
                    .GetValue()
                    .ok()
                    .and_then(|v| i32::try_from(&v).ok())
                    != Some(value)
                {
                    let _ = compartment.SetValue(
                        self.tid.load(Ordering::SeqCst),
                        &windows::Win32::System::Variant::VARIANT::from(value),
                    );
                }
            }
        }
        let sink = lock(&self.language_bar).as_ref().and_then(|bar| bar.sink());
        if let Some(sink) = sink {
            // SAFETY: Callback is on the owning TSF apartment, outside state locks.
            unsafe {
                let _ = sink.OnUpdate(TF_LBI_ICON | TF_LBI_TEXT | TF_LBI_STATUS);
            }
        }
    }
    pub(crate) fn sync_scheme(&self) {
        if lock(&self.composition).is_some() || self.pending.load(Ordering::SeqCst) != 0 {
            return;
        }
        if let Some(session) = self.session() {
            session.sync_packs();
            let scheme = crate::preferences::scheme();
            let (enabled, spelling) = crate::preferences::english_options();
            if session
                .backend
                .with_kernel(|k| (k.config().english_enabled, k.config().english_spelling))
                != (enabled, spelling)
            {
                session.submit(InputEvent::SetEnglishOptions { enabled, spelling });
            }
            if session.backend.with_kernel(|k| k.config().pinyin_scheme) != scheme {
                session.submit(InputEvent::SetPinyinScheme(scheme));
                self.notify_language_bar();
            }
        }
    }
    pub(crate) fn hide(&self) {
        let window = lock(&self.window).take();
        if let Some(mut window) = window {
            window.hide();
            *lock(&self.window) = Some(window);
        }
    }
    pub(crate) fn reset_kernel(&self) {
        if let Some(s) = self.session() {
            s.submit(InputEvent::ResetComposition);
        }
    }
    pub(crate) fn finish(self: &Arc<Self>, cancel: bool) {
        let composition = lock(&self.composition).clone();
        if let Some(c) = composition {
            let _ = edit::request(self, &c.context, edit::Work::Finish(cancel));
        } else {
            self.reset_kernel();
        }
        self.hide();
    }
    pub(crate) fn wants(&self, key: Key, mods: Modifiers) -> bool {
        let Some(session) = self.session() else {
            return false;
        };
        if session.shadow {
            return false;
        }
        let (chinese, composing, english, candidates, selected) =
            session.backend.with_kernel(|k| {
                (
                    k.is_chinese(),
                    k.has_composition(),
                    k.config().english_enabled,
                    k.has_candidates(),
                    k.english_candidate_selected(),
                )
            });
        let composing = composing || self.pending.load(Ordering::SeqCst) > 0;
        if !chinese && english {
            if !mods.is_plain() {
                return false;
            }
            return matches!(key,Key::Char(c) if c.is_ascii_alphabetic())
                || composing
                    && (matches!(
                        key,
                        Key::Char(_) | Key::Space | Key::Escape | Key::Backspace
                    ) || candidates
                        && (matches!(key, Key::Up | Key::Down)
                            || key == Key::Tab && !mods.contains(Modifiers::SHIFT))
                        || selected && matches!(key, Key::PageUp | Key::PageDown));
        }
        wants_key(key, mods, chinese, composing)
    }
}
fn wants_key(key: Key, mods: Modifiers, chinese: bool, composing: bool) -> bool {
    if !mods.is_plain() || !chinese {
        return false;
    }
    if composing {
        return matches!(
            key,
            Key::Char(_)
                | Key::Space
                | Key::Enter
                | Key::Escape
                | Key::Backspace
                | Key::Left
                | Key::Right
                | Key::Up
                | Key::Down
                | Key::PageUp
                | Key::PageDown
        );
    }
    matches!(key, Key::Char(c) if
        (c.is_ascii_lowercase() && !mods.contains(Modifiers::SHIFT)) ||
        retype_engine::kernel::chinese_punctuation(c).is_some())
}
#[implement(
    ITfTextInputProcessorEx,
    ITfDisplayAttributeProvider,
    ITfFunctionProvider
)]
pub struct RetypeTip {
    pub state: Arc<TipState>,
}
impl RetypeTip {
    pub fn create() -> ITfTextInputProcessorEx {
        Self {
            state: TipState::new(),
        }
        .into()
    }
}
impl ITfTextInputProcessor_Impl for RetypeTip_Impl {
    fn Activate(&self, mgr: Ref<'_, ITfThreadMgr>, tid: u32) -> Result<()> {
        guarded(|| self.state.activate(mgr, tid, 0))
    }
    fn Deactivate(&self) -> Result<()> {
        guarded(|| self.state.deactivate())
    }
}
impl ITfTextInputProcessorEx_Impl for RetypeTip_Impl {
    fn ActivateEx(&self, mgr: Ref<'_, ITfThreadMgr>, tid: u32, flags: u32) -> Result<()> {
        guarded(|| self.state.activate(mgr, tid, flags))
    }
}

impl ITfFunctionProvider_Impl for RetypeTip_Impl {
    fn GetType(&self) -> Result<GUID> {
        Ok(crate::ids::CLSID_RETYPE_TIP)
    }

    fn GetDescription(&self) -> Result<windows_core::BSTR> {
        Ok(windows_core::BSTR::from("retype search candidates"))
    }

    fn GetFunction(&self, kind: *const GUID, iid: *const GUID) -> Result<windows_core::IUnknown> {
        crate::search::function(&self.state, kind, iid)
    }
}
impl ITfDisplayAttributeProvider_Impl for RetypeTip_Impl {
    fn EnumDisplayAttributeInfo(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(display::enumeration())
    }
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // COM trait fixes this signature.
    fn GetDisplayAttributeInfo(&self, guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
        if guid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: COM caller provides a valid GUID pointer.
        if unsafe { *guid } != display::ATTRIBUTE {
            return Err(E_INVALIDARG.into());
        }
        Ok(display::info())
    }
}
#[implement(ITfThreadFocusSink, ITfThreadMgrEventSink)]
struct FocusSink {
    state: Weak<TipState>,
}
impl ITfThreadMgrEventSink_Impl for FocusSink_Impl {
    fn OnInitDocumentMgr(&self, _doc: Ref<'_, ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }
    fn OnUninitDocumentMgr(&self, doc: Ref<'_, ITfDocumentMgr>) -> Result<()> {
        self.OnSetFocus(Ref::default(), doc)
    }
    fn OnSetFocus(
        &self,
        focused: Ref<'_, ITfDocumentMgr>,
        _previous: Ref<'_, ITfDocumentMgr>,
    ) -> Result<()> {
        guarded(|| {
            if let Some(state) = self.state.upgrade() {
                lock(&state.stats_clock).reset();
                let current = lock(&state.composition).clone();
                if let Some(current) = current {
                    // SAFETY: Read document identity only; writes are requested separately.
                    let owner = unsafe { current.context.GetDocumentMgr() };
                    if !owner.is_ok_and(|owner| focused.ok().is_ok_and(|focus| owner == *focus)) {
                        state.epoch.fetch_add(1, Ordering::SeqCst);
                        state.finish(false);
                    }
                }
            }
            Ok(())
        })
    }
    fn OnPushContext(&self, _ctx: Ref<'_, ITfContext>) -> Result<()> {
        Ok(())
    }
    fn OnPopContext(&self, ctx: Ref<'_, ITfContext>) -> Result<()> {
        guarded(|| {
            if let Some(state) = self.state.upgrade() {
                let current = lock(&state.composition).clone();
                if current.is_some_and(|c| ctx.ok().is_ok_and(|ctx| c.context == *ctx)) {
                    state.epoch.fetch_add(1, Ordering::SeqCst);
                    state.finish(false);
                }
            }
            Ok(())
        })
    }
}
impl ITfThreadFocusSink_Impl for FocusSink_Impl {
    fn OnSetThreadFocus(&self) -> Result<()> {
        if let Some(state) = self.state.upgrade() {
            state.shortcuts();
            state.sync_scheme();
        }
        Ok(())
    }
    fn OnKillThreadFocus(&self) -> Result<()> {
        guarded(|| {
            if let Some(state) = self.state.upgrade() {
                *lock(&state.shift_tap) = ShiftTap::default();
                lock(&state.stats_clock).reset();
                state.epoch.fetch_add(1, Ordering::SeqCst);
                state.finish(false);
            }
            Ok(())
        })
    }
}
#[implement(ITfKeyEventSink)]
pub struct KeyEventSink {
    pub state: Weak<TipState>,
}
impl KeyEventSink_Impl {
    fn key(&self, ctx: Ref<'_, ITfContext>, vk: WPARAM, lp: LPARAM, test: bool) -> Result<BOOL> {
        if settings_host() {
            return Ok(false.into());
        }
        let Some(state) = self.state.upgrade() else {
            return Ok(false.into());
        };
        if !state.is_activated() {
            return Ok(false.into());
        }
        let Ok(ctx) = ctx.ok() else {
            return Ok(false.into());
        };
        // SAFETY: Reading context status does not acquire a document write lock.
        let Ok(status) = (unsafe { ctx.GetStatus() }) else {
            return Ok(false.into());
        };
        if status.dwDynamicFlags & TS_SD_READONLY != 0 {
            return Ok(false.into());
        }
        if crate::voice::active(&state) && vk.0 == 0x1b {
            if !test {
                crate::voice::dismiss(&state);
            }
            return Ok(true.into());
        }
        let bindings = state.shortcuts();
        let bits = shortcut_modifiers(keymap::read_modifiers());
        let matches = |binding: retype_ai::config::Shortcut| {
            binding.vk != 0
                && !binding.is_tap()
                && binding.vk == (vk.0 & 0xffff) as u16
                && binding.modifiers == bits
        };
        let work = if voice_matches(bindings.voice, vk, lp, bits) {
            Some(edit::Work::Voice(true))
        } else if matches(bindings.translate) {
            Some(edit::Work::Translate)
        } else if matches(bindings.mode) {
            Some(edit::Work::Toggle)
        } else {
            None
        };
        if !test && work.is_none() && !matches!(vk.0, 0x10 | 0x11 | 0x12 | 0xa0..=0xa5) {
            crate::voice::dismiss(&state);
        }
        if let Some(work) = work {
            if !test {
                lock(&state.shift_tap).other_key();
            }
            if test || lp.0 & (1 << 30) != 0 {
                return Ok(true.into());
            }
            let result = edit::request(&state, ctx, work);
            if matches!(work, edit::Work::Voice(true))
                && bindings.voice == retype_ai::config::Shortcut::VOICE
                && result.is_ok()
            {
                state.voice_alt_down.store(true, Ordering::Release);
            }
            if matches!(work, edit::Work::Translate) {
                crate::settings_log::event(
                    "launcher",
                    "translation_shortcut",
                    0,
                    format!(
                        "accepted={} hresult={}",
                        result.is_ok(),
                        result.as_ref().err().map_or(0, |e| e.code().0)
                    ),
                );
            }
            return Ok(result.is_ok().into());
        }
        let Some(key) =
            keymap::translate_event((vk.0 & 0xffff) as u16, ((lp.0 >> 16) & 0xff) as u32)
        else {
            return Ok(false.into());
        };
        let mut mods = keymap::read_modifiers();
        // The kernel treats shifted letters as raw English. CapsLock uses the same path.
        if matches!(key, Key::Char(c) if c.is_ascii_uppercase()) {
            mods = mods.union(Modifiers::SHIFT);
        }
        let wanted = state.wants(key, mods);
        let english = state.session().is_some_and(|session| {
            session
                .backend
                .with_kernel(|k| !k.is_chinese() && k.config().english_enabled)
        });
        if english && status.dwStaticFlags & TS_SS_NOHIDDENTEXT == 0 {
            return Ok(false.into());
        }
        let direct = !wanted
            && status.dwStaticFlags & TS_SS_NOHIDDENTEXT != 0
            && state.session().is_some_and(|session| !session.shadow)
            && mods.is_plain()
            && matches!(key, Key::Char(ch) if ch.is_ascii_alphabetic());
        if test {
            return Ok((wanted || direct).into());
        }
        if direct {
            let Key::Char(ch) = key else {
                return Ok(false.into());
            };
            // English text is counted only if the synchronous TSF write succeeded.
            // If a host refuses a synchronous edit, let it handle the key normally.
            return Ok(edit::request(&state, ctx, edit::Work::Direct(ch))
                .is_ok()
                .into());
        }
        if !wanted {
            if status.dwStaticFlags & TS_SS_NOHIDDENTEXT != 0 {
                if key == Key::Backspace {
                    stats::backspace(&state.stats_clock);
                } else if mods.is_plain()
                    && matches!(key, Key::Char(ch) if ch.is_ascii_digit() || matches!(ch, '\'' | '\u{2019}'))
                {
                    if let Key::Char(ch) = key {
                        // Passthrough connectors have no quantity of their own;
                        // retain the boundary state for direct English input.
                        stats::commit(&state.stats_clock, &ch.to_string(), false);
                    }
                } else {
                    stats::boundary(&state.stats_clock);
                }
            }
            if english
                && state
                    .session()
                    .is_some_and(|session| session.backend.with_kernel(|k| k.has_composition()))
            {
                // Flush the word synchronously, then allow the real key to reach
                // the host (Enter submits searches; arrows and shortcuts stay native).
                let _ = edit::request(&state, ctx, edit::Work::Boundary(key, mods));
                return Ok(false.into());
            }
            // End existing composition before shortcuts, navigation or English passthrough.
            state.finish(false);
            return Ok(false.into());
        }
        match edit::request(&state, ctx, edit::Work::Key(key, mods)) {
            Ok(()) => {
                if status.dwStaticFlags & TS_SS_NOHIDDENTEXT != 0 {
                    stats::activity(
                        &state.stats_clock,
                        if english {
                            stats::Language::English
                        } else {
                            stats::Language::Chinese
                        },
                    );
                }
                Ok(true.into())
            }
            Err(e) => {
                tracing::warn!("TSF edit request rejected: {e}");
                Ok(false.into())
            }
        }
    }
}
impl ITfKeyEventSink_Impl for KeyEventSink_Impl {
    fn OnSetFocus(&self, foreground: BOOL) -> Result<()> {
        guarded(|| {
            if foreground.as_bool() {
                if let Some(state) = self.state.upgrade() {
                    state.shortcuts();
                    state.sync_scheme();
                }
            }
            if !foreground.as_bool() {
                if let Some(s) = self.state.upgrade() {
                    s.voice_alt_down.store(false, Ordering::Release);
                    *lock(&s.shift_tap) = ShiftTap::default();
                    s.epoch.fetch_add(1, Ordering::SeqCst);
                    s.finish(false);
                }
            }
            Ok(())
        })
    }
    fn OnTestKeyDown(&self, ctx: Ref<'_, ITfContext>, vk: WPARAM, lp: LPARAM) -> Result<BOOL> {
        if settings_host() {
            return Ok(false.into());
        }
        guarded(|| {
            let Some(state) = self.state.upgrade() else {
                return Ok(false.into());
            };
            let binding = state.shortcuts().mode;
            if binding.is_tap() && normalized_modifier(vk) == binding.vk {
                return Ok(state.is_activated().into());
            }
            lock(&state.shift_tap).other_key();
            self.key(ctx, vk, lp, true)
        })
    }
    fn OnKeyDown(&self, ctx: Ref<'_, ITfContext>, vk: WPARAM, lp: LPARAM) -> Result<BOOL> {
        if settings_host() {
            return Ok(false.into());
        }
        guarded(|| {
            if let Some(state) = self.state.upgrade() {
                let binding = state.shortcuts().mode;
                if binding.is_tap() && normalized_modifier(vk) == binding.vk {
                    if state.is_activated() {
                        lock(&state.shift_tap).press(tap_modifiers(binding.vk));
                    }
                    return Ok(false.into());
                }
                lock(&state.shift_tap).other_key();
            }
            self.key(ctx, vk, lp, false)
        })
    }
    fn OnTestKeyUp(&self, _ctx: Ref<'_, ITfContext>, vk: WPARAM, lp: LPARAM) -> Result<BOOL> {
        Ok((self.state.upgrade().is_some_and(|state| {
            if right_alt_event(vk, lp) && state.voice_alt_down.load(Ordering::Acquire) {
                return true;
            }
            let binding = state.shortcuts().mode;
            binding.is_tap()
                && normalized_modifier(vk) == binding.vk
                && state.is_activated()
                && lock(&state.shift_tap).pressed
        }))
        .into())
    }
    fn OnKeyUp(&self, ctx: Ref<'_, ITfContext>, vk: WPARAM, lp: LPARAM) -> Result<BOOL> {
        if let Some(state) = self.state.upgrade() {
            if right_alt_event(vk, lp) && state.voice_alt_down.swap(false, Ordering::AcqRel) {
                crate::voice::release_hold(&state);
                return Ok(true.into());
            }
            let binding = state.shortcuts().mode;
            if binding.is_tap() && normalized_modifier(vk) == binding.vk {
                let toggle = lock(&state.shift_tap).release(tap_modifiers(binding.vk));
                if toggle && state.is_activated() {
                    if let Ok(ctx) = ctx.ok() {
                        if let Err(error) = edit::request(&state, ctx, edit::Work::Toggle) {
                            tracing::warn!("Shift mode switch rejected: {error}");
                        }
                    }
                }
            }
        }
        // Pass the key-up to the host so it never sees a stuck Shift key.
        Ok(false.into())
    }
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // COM fixes this callback signature.
    fn OnPreservedKey(&self, ctx: Ref<'_, ITfContext>, guid: *const GUID) -> Result<BOOL> {
        guarded(|| {
            if guid.is_null() || settings_host() {
                return Ok(false.into());
            }
            // SAFETY: TSF supplies a valid command GUID for this callback.
            if unsafe { *guid } != TRANSLATION_COMMAND && unsafe { *guid } != VOICE_COMMAND {
                return Ok(false.into());
            }
            let Some(state) = self.state.upgrade().filter(|state| state.is_activated()) else {
                return Ok(false.into());
            };
            let voice = unsafe { *guid } == VOICE_COMMAND;
            let bindings = state.shortcuts();
            let binding = if voice {
                bindings.voice
            } else {
                bindings.translate
            };
            if voice
                && binding == retype_ai::config::Shortcut::VOICE
                && shortcut_modifiers(keymap::read_modifiers()) != 2
            {
                return Ok(false.into());
            }
            if *lock(if voice {
                &state.preserved_voice
            } else {
                &state.preserved_translation
            }) != Some(binding)
            {
                return Ok(false.into());
            }
            let Ok(ctx) = ctx.ok() else {
                return Ok(false.into());
            };
            lock(&state.shift_tap).other_key();
            let result = edit::request(
                &state,
                ctx,
                if voice {
                    edit::Work::Voice(true)
                } else {
                    edit::Work::Translate
                },
            );
            if voice && binding == retype_ai::config::Shortcut::VOICE && result.is_ok() {
                state.voice_alt_down.store(true, Ordering::Release);
            }
            crate::settings_log::event(
                "launcher",
                "translation_preserved_shortcut",
                0,
                format!(
                    "accepted={} hresult={}",
                    result.is_ok(),
                    result.as_ref().err().map_or(0, |e| e.code().0)
                ),
            );
            Ok(result.is_ok().into())
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn voice_hold_distinguishes_right_alt_and_excludes_other_chords() {
        use retype_ai::config::Shortcut;
        let right = LPARAM(1 << 24);
        assert!(voice_matches(Shortcut::VOICE, WPARAM(0x12), right, 2));
        assert!(voice_matches(Shortcut::VOICE, WPARAM(0xa5), LPARAM(0), 2));
        assert!(!voice_matches(Shortcut::VOICE, WPARAM(0x12), LPARAM(0), 2));
        assert!(!voice_matches(Shortcut::VOICE, WPARAM(0xa4), right, 2));
        for bits in [0, 3, 6, 10] {
            assert!(!voice_matches(Shortcut::VOICE, WPARAM(0x12), right, bits));
        }
        assert_eq!(
            preserved_voice(Shortcut::VOICE).map(|k| (k.uVKey, k.uModifiers)),
            Some((0x12, TF_MOD_RALT))
        );
        assert!(!right_alt_event(WPARAM(0x41), right));
    }
    #[test]
    fn preserved_translation_uses_exact_modifiers_and_excludes_disabled_and_win() {
        use retype_ai::config::Shortcut;
        let key = preserved_translation(Shortcut::TRANSLATE).map(|key| (key.uVKey, key.uModifiers));
        assert_eq!(key, Some((0x30, TF_MOD_CONTROL | TF_MOD_ALT)));
        let key = preserved_translation(Shortcut {
            vk: 0x54,
            modifiers: 5,
        })
        .map(|key| key.uModifiers);
        assert_eq!(key, Some(TF_MOD_CONTROL | TF_MOD_SHIFT));
        assert!(preserved_translation(Shortcut::DISABLED).is_none());
        assert!(preserved_translation(Shortcut {
            vk: 0x54,
            modifiers: 9
        })
        .is_none());
    }
    #[test]
    fn tip_state_starts_inactive() {
        let s = TipState::new();
        assert!(!s.is_activated());
        assert!(s.session().is_none());
    }
    #[test]
    fn deactivate_without_activate_is_harmless() {
        assert!(TipState::new().deactivate().is_ok());
    }
    #[test]
    fn keyboard_routing_preserves_shortcuts_and_idle_keys() {
        assert!(!wants_key(Key::Char('a'), Modifiers::CTRL, true, true));
        assert!(!wants_key(Key::Space, Modifiers::NONE, true, false));
        assert!(!wants_key(Key::Backspace, Modifiers::NONE, true, false));
        assert!(!wants_key(Key::Char('a'), Modifiers::NONE, false, false));
        assert!(wants_key(Key::Char('a'), Modifiers::NONE, true, false));
        assert!(wants_key(Key::Char('2'), Modifiers::NONE, true, true));
        assert!(!wants_key(Key::Space, Modifiers::CTRL, false, false));
        assert!(!wants_key(Key::Char('a'), Modifiers::SHIFT, true, false));
        assert!(wants_key(Key::Char('='), Modifiers::NONE, true, true));
        assert!(wants_key(Key::Char('-'), Modifiers::NONE, true, true));
        assert!(wants_key(Key::Char(','), Modifiers::NONE, true, false));
        assert!(wants_key(Key::Char('?'), Modifiers::SHIFT, true, false));
        assert!(!wants_key(Key::Char(','), Modifiers::NONE, false, false));
        assert!(!wants_key(Key::Char(','), Modifiers::CTRL, true, false));
    }

    #[test]
    fn shift_tap_only_toggles_when_released_alone() {
        let mut shift = ShiftTap::default();
        shift.press(Modifiers::SHIFT);
        assert!(shift.release(Modifiers::NONE));
        shift.press(Modifiers::SHIFT);
        shift.other_key();
        assert!(!shift.release(Modifiers::NONE));
        shift.press(Modifiers::SHIFT.union(Modifiers::CTRL));
        assert!(!shift.release(Modifiers::NONE));
        shift.press(Modifiers::SHIFT);
        assert!(!shift.release(Modifiers::CTRL));
    }
}
