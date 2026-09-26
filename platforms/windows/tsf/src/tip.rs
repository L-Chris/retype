//! Windows TSF lifecycle and keyboard routing. Text changes run under TSF edit locks.
use crate::{candidate::CandidateWindow, display, edit, keymap, session::Session};

use retype_types::{InputEvent, InputSource, Key, Modifiers};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use windows::Win32::Foundation::{E_INVALIDARG, E_POINTER, E_UNEXPECTED, LPARAM, WPARAM};
use windows::Win32::UI::TextServices::*;
use windows_core::{implement, Interface, Ref, Result, BOOL, GUID};
const TOGGLE_KEY: GUID = GUID::from_u128(0x9adc1a31_2426_4c80_95f3_79bb71d8932e);
const TOGGLE_CHORD: TF_PRESERVEDKEY = TF_PRESERVEDKEY {
    uVKey: 0x20,
    uModifiers: TF_MOD_CONTROL,
};

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
    language_bar: Mutex<Option<crate::langbar::LanguageBar>>,
    preserved_key: AtomicBool,
    mode_bridge: Mutex<Option<crate::langbar::ModeBridge>>,
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
            language_bar: Mutex::new(None),
            preserved_key: AtomicBool::new(false),
            mode_bridge: Mutex::new(None),
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
        if let Ok(bridge) = crate::langbar::ModeBridge::attach(self, &mgr) {
            *lock(&self.mode_bridge) = Some(bridge);
        }
        // SAFETY: TSF dispatches this shortcut through OnPreservedKey even in hosts
        // that reserve Ctrl+Space before normal key callbacks (for example RichEdit).
        unsafe {
            if let Ok(keys) = mgr.cast::<ITfKeystrokeMgr>() {
                self.preserved_key.store(
                    keys.PreserveKey(
                        tid,
                        &TOGGLE_KEY,
                        &TOGGLE_CHORD,
                        &"retype 中英切换".encode_utf16().collect::<Vec<_>>(),
                    )
                    .is_ok(),
                    Ordering::SeqCst,
                );
            }
        }
        match crate::langbar::LanguageBar::attach(self, &mgr) {
            Ok(bar) => *lock(&self.language_bar) = Some(bar),
            Err(error) => {
                let _ = self.deactivate();
                return Err(error);
            }
        }
        Ok(())
    }
    pub fn deactivate(self: &Arc<Self>) -> Result<()> {
        self.activated.store(false, Ordering::SeqCst);
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
                    if self.preserved_key.swap(false, Ordering::SeqCst) {
                        let _ = keys.UnpreserveKey(&TOGGLE_KEY, &TOGGLE_CHORD);
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
            let scheme = crate::preferences::scheme();
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
            s.submit(InputEvent::Key {
                key: Key::Escape,
                mods: Modifiers::NONE,
                source: InputSource::Keyboard,
            });
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
        let (chinese, composing) = session
            .backend
            .with_kernel(|k| (k.is_chinese(), k.has_composition()));
        let composing = composing || self.pending.load(Ordering::SeqCst) > 0;
        wants_key(key, mods, chinese, composing)
    }
}
fn wants_key(key: Key, mods: Modifiers, chinese: bool, composing: bool) -> bool {
    if key == Key::Space && mods == Modifiers::CTRL {
        return true;
    }
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
    matches!(key, Key::Char(c) if c.is_ascii_lowercase()) && !mods.contains(Modifiers::SHIFT)
}
#[implement(ITfTextInputProcessorEx, ITfDisplayAttributeProvider)]
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
            state.sync_scheme();
        }
        Ok(())
    }
    fn OnKillThreadFocus(&self) -> Result<()> {
        guarded(|| {
            if let Some(state) = self.state.upgrade() {
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
        if unsafe { ctx.GetStatus() }
            .map(|s| s.dwDynamicFlags & TS_SD_READONLY != 0)
            .unwrap_or(true)
        {
            return Ok(false.into());
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
        if test {
            return Ok(wanted.into());
        }
        if !wanted {
            // End existing composition before shortcuts, navigation or English passthrough.
            state.finish(false);
            return Ok(false.into());
        }
        let work = if key == Key::Space && mods == Modifiers::CTRL {
            edit::Work::Toggle
        } else {
            edit::Work::Key(key, mods)
        };
        match edit::request(&state, ctx, work) {
            Ok(()) => Ok(true.into()),
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
                    state.sync_scheme();
                }
            }
            if !foreground.as_bool() {
                if let Some(s) = self.state.upgrade() {
                    s.epoch.fetch_add(1, Ordering::SeqCst);
                    s.finish(false);
                }
            }
            Ok(())
        })
    }
    fn OnTestKeyDown(&self, ctx: Ref<'_, ITfContext>, vk: WPARAM, lp: LPARAM) -> Result<BOOL> {
        guarded(|| self.key(ctx, vk, lp, true))
    }
    fn OnKeyDown(&self, ctx: Ref<'_, ITfContext>, vk: WPARAM, lp: LPARAM) -> Result<BOOL> {
        guarded(|| self.key(ctx, vk, lp, false))
    }
    fn OnTestKeyUp(&self, _ctx: Ref<'_, ITfContext>, _vk: WPARAM, _lp: LPARAM) -> Result<BOOL> {
        Ok(false.into())
    }
    fn OnKeyUp(&self, _ctx: Ref<'_, ITfContext>, _vk: WPARAM, _lp: LPARAM) -> Result<BOOL> {
        Ok(false.into())
    }
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // COM fixes this callback signature.
    fn OnPreservedKey(&self, ctx: Ref<'_, ITfContext>, guid: *const GUID) -> Result<BOOL> {
        guarded(|| {
            if guid.is_null() {
                return Ok(false.into());
            }
            // SAFETY: TSF supplies a valid command GUID.
            if unsafe { *guid } != TOGGLE_KEY {
                return Ok(false.into());
            }
            let Some(state) = self.state.upgrade() else {
                return Ok(false.into());
            };
            if !state.is_activated() {
                return Ok(false.into());
            }
            Ok(edit::request(&state, ctx.ok()?, edit::Work::Toggle)
                .is_ok()
                .into())
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(wants_key(Key::Space, Modifiers::CTRL, false, false));
        assert!(!wants_key(Key::Char('a'), Modifiers::SHIFT, true, false));
    }
}
