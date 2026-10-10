//! Composition transactions. Never write a host document outside DoEditSession.
use crate::stats::{self, Language};
use crate::tip::{guarded, lock, TipState};

use retype_types::{
    CommitRequest, InputEvent, InputSource, KernelAction, Key, Modifiers, SideEffect,
};
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Weak};
use std::time::Instant;
use windows::Win32::Foundation::{E_FAIL, RECT};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::TextServices::*;
use windows_core::{implement, Interface, Ref, Result, BOOL};

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;

#[derive(Clone)]
pub(crate) struct Composition {
    pub context: ITfContext,
    object: ITfComposition,
    source: ITfSource,
    cookie: u32,
    layout_cookie: u32,
}
#[derive(Clone, Copy)]
pub(crate) enum Work {
    Key(Key, Modifiers),
    Boundary(Key, Modifiers),
    Direct(char),
    PairBackspace,
    Toggle,
    SetChinese(bool),
    Choose(usize, u64),
    Finish(bool),
    Refresh,
    Translate,
    Voice(bool),
}

impl Work {
    fn profile_name(self) -> &'static str {
        match self {
            Self::Key(Key::Backspace, _) => "backspace",
            Self::PairBackspace => "symbol_backspace",
            Self::Key(Key::Char(_), _) => "typing",
            Self::Key(_, _) => "navigation",
            Self::Refresh => "layout_refresh",
            Self::Choose(..) => "choose",
            Self::Finish(_) => "finish",
            Self::Direct(_) | Self::Boundary(..) => "boundary",
            Self::Toggle | Self::SetChinese(_) => "mode",
            Self::Translate => "translation",
            Self::Voice(_) => "voice",
        }
    }
}

pub(crate) fn request(state: &Arc<TipState>, context: &ITfContext, work: Work) -> Result<()> {
    static REFRESH_SERIAL: AtomicU32 = AtomicU32::new(1);
    let epoch = state.epoch.load(Ordering::SeqCst);
    let refresh_id = if matches!(work, Work::Refresh) {
        let mut pending = lock(&state.refresh_pending);
        if pending.is_some_and(|(queued_epoch, _)| queued_epoch == epoch) {
            return Ok(());
        }
        let id = REFRESH_SERIAL.fetch_add(1, Ordering::Relaxed).max(1);
        *pending = Some((epoch, id));
        id
    } else {
        0
    };
    let synchronous = matches!(
        work,
        Work::Direct(_) | Work::Boundary(..) | Work::PairBackspace
    ) || matches!(work, Work::Key(Key::Char(c), _) if !c.is_ascii_alphanumeric()
            && state.pending.load(Ordering::SeqCst) == 0
            && state.session().is_some_and(|s| s.backend.with_kernel(|k| {
                let rule = retype_types::symbols::rules(c, k.is_chinese());
                !k.has_composition() && (rule.opening.is_some() || rule.closing.is_some())
            })) && crate::preferences::symbol_completion());
    let pending_counted = !synchronous;
    let cancelled = Arc::new(AtomicBool::new(false));
    let completed = Arc::new(AtomicBool::new(false));
    let session: ITfEditSession = Edit {
        state: Arc::clone(state),
        context: context.clone(),
        work,
        epoch,
        finishing: if matches!(work, Work::Finish(_)) {
            lock(&state.composition).as_ref().map(|c| c.object.clone())
        } else {
            None
        },
        cancelled: Arc::clone(&cancelled),
        completed: Arc::clone(&completed),
        pending_counted,
        synchronous,
        requested: Instant::now(),
        refresh_id,
    }
    .into();
    if pending_counted {
        state.pending.fetch_add(1, Ordering::SeqCst);
    }
    // SAFETY: TSF holds the COM edit session if deferred. Kernel input is not consumed
    // until a write lock is granted. ASYNCDONTCARE allows immediate or deferred execution.
    unsafe {
        let status = context.RequestEditSession(
            state.tid.load(Ordering::SeqCst),
            &session,
            if synchronous {
                TF_ES_SYNC
            } else {
                TF_ES_ASYNCDONTCARE
            } | if matches!(work, Work::Refresh) {
                TF_ES_READ
            } else {
                TF_ES_READWRITE
            },
        )?;
        if synchronous && !completed.load(Ordering::SeqCst) {
            // Some hosts return an asynchronous success even for TF_ES_SYNC.
            // Cancel that deferred session and let the host receive the key.
            cancelled.store(true, Ordering::SeqCst);
            return Err(E_FAIL.into());
        }
        status.ok()
    }
}
#[implement(ITfEditSession)]
struct Edit {
    state: Arc<TipState>,
    context: ITfContext,
    work: Work,
    epoch: u32,
    finishing: Option<ITfComposition>,
    cancelled: Arc<AtomicBool>,
    completed: Arc<AtomicBool>,
    pending_counted: bool,
    synchronous: bool,
    requested: Instant,
    refresh_id: u32,
}
impl Edit {
    fn release_refresh(&self) {
        if self.refresh_id != 0 {
            let mut pending = lock(&self.state.refresh_pending);
            if *pending == Some((self.epoch, self.refresh_id)) {
                *pending = None;
            }
        }
    }
}
impl Drop for Edit {
    fn drop(&mut self) {
        self.release_refresh();
        if self.pending_counted {
            self.state.pending.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
impl ITfEditSession_Impl for Edit_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        let mut timing = crate::input_profile::Timing::new(
            self.requested,
            self.work.profile_name(),
            self.state.pending.load(Ordering::SeqCst),
        );
        let result = guarded(|| {
            if self.cancelled.load(Ordering::SeqCst) {
                return Ok(());
            }
            if !matches!(self.work, Work::Finish(_))
                && (!self.state.is_activated()
                    || self.epoch != self.state.epoch.load(Ordering::SeqCst))
            {
                return Ok(());
            }
            if matches!(self.work, Work::Refresh) {
                return self.refresh(ec, &mut timing);
            }
            self.state.writing.store(true, Ordering::SeqCst);
            let result = guarded(|| self.run(ec, &mut timing));
            self.state.writing.store(false, Ordering::SeqCst);
            if result.is_ok() && self.synchronous {
                self.completed.store(true, Ordering::SeqCst);
            }
            if result.is_err() && !matches!(self.work, Work::PairBackspace) {
                lock(&self.state.symbol_pairs).clear();
                // Fail closed: leave existing document text intact, release composition ownership.
                let current = lock(&self.state.composition).clone();
                if current.as_ref().is_some_and(|c| c.context == self.context) {
                    let _ = end(&self.state, ec, false);
                }
                self.state.reset_kernel();
                self.state.hide();
                lock(&self.state.stats_clock).reset();
            }
            result
        });
        // Some hosts retain the completed COM session. Release now as well as
        // on Drop; the token prevents an older session clearing a newer one.
        self.release_refresh();
        timing.finish(
            || {
                self.state
                    .session()
                    .map(|session| session.backend.with_kernel(|k| k.input_metrics()))
                    .unwrap_or_default()
            },
            result.as_ref().err().map_or(0, |e| e.code().0),
        );
        result
    }
}
impl Edit_Impl {
    fn run(&self, ec: u32, timing: &mut crate::input_profile::Timing) -> Result<()> {
        let state = &self.state;
        if matches!(self.work, Work::PairBackspace) {
            return if crate::preferences::symbol_completion()
                && learnable(&self.context, ec)
                && crate::symbols::backspace(state, &self.context, ec)?
            {
                stats::backspace(&state.stats_clock);
                Ok(())
            } else {
                Err(E_FAIL.into())
            };
        }
        if matches!(self.work, Work::Refresh) {
            return self.refresh(ec, timing);
        }
        if let Work::Finish(cancel) = self.work {
            let c = lock(&state.composition).clone();
            if c.as_ref().is_some_and(|c| {
                c.context == self.context && self.finishing.as_ref() == Some(&c.object)
            }) {
                let english = state.session().and_then(|session| {
                    session
                        .backend
                        .with_kernel(|k| (!k.is_chinese() && !cancel).then(|| k.composition_text()))
                });
                end(state, ec, cancel)?;
                state.reset_kernel();
                if let Some(text) = english {
                    if countable(&self.context) {
                        stats::commit(&state.stats_clock, &text, true);
                    }
                    if learnable(&self.context, ec)
                        && text.len() >= 2
                        && text.len() <= 64
                        && text.bytes().all(|c| c.is_ascii_alphabetic() || c == b'\'')
                    {
                        if let Some(session) = state.session() {
                            session
                                .backend
                                .record_learning(retype_types::LearningEvent::EnglishWord { text });
                        }
                    }
                }
            }
            return Ok(());
        }
        if let Work::Direct(ch) = self.work {
            let text = ch.to_string();
            replace(state, &self.context, ec, &text, false)?;
            if countable(&self.context) {
                stats::activity(&state.stats_clock, Language::English);
                stats::commit(&state.stats_clock, &text, false);
            }
            return Ok(());
        }
        let Some(session) = state.session() else {
            return Ok(());
        };
        if matches!(self.work, Work::Key(..) | Work::Boundary(..))
            && session.backend.with_kernel(|k| !k.is_chinese())
            && !learnable(&self.context, ec)
        {
            // Hidden/private/password scopes receive literal text, with neither
            // predictions nor personal learning. Do not read surrounding text.
            let literal = match self.work {
                Work::Key(Key::Char(c), _) => Some(c.to_string()),
                Work::Key(Key::Space, _) => Some(" ".into()),
                _ => None,
            };
            end(state, ec, false)?;
            state.reset_kernel();
            state.hide();
            if let Some(text) = literal {
                replace(state, &self.context, ec, &text, false)?;
            }
            return Ok(());
        }
        if matches!(self.work, Work::Translate | Work::Voice(_))
            && !session.backend.with_kernel(|k| k.has_composition())
        {
            stats::boundary(&state.stats_clock);
            return if let Work::Voice(held) = self.work {
                crate::voice::capture(state, &self.context, ec, held)
            } else {
                crate::translation::capture(state, &self.context, ec)
            };
        }
        let existing = lock(&state.composition).clone();
        if existing.as_ref().is_some_and(|c| c.context != self.context) {
            // Cannot edit an old context with a cookie issued for a different document.
            state.finish(false);
            return Err(E_FAIL.into());
        }
        let event = match self.work {
            Work::Translate | Work::Voice(_) => InputEvent::Key {
                key: if session.backend.with_kernel(|k| k.is_chinese()) {
                    Key::Space
                } else {
                    Key::Enter
                },
                mods: Modifiers::NONE,
                source: InputSource::Keyboard,
            },
            Work::Choose(index, generation) => {
                if session.backend.with_kernel(|k| k.generation()) != generation {
                    return Ok(());
                }
                InputEvent::CandidateChosen { index }
            }
            Work::Key(key, mods) | Work::Boundary(key, mods) => InputEvent::Key {
                key,
                mods,
                source: InputSource::Keyboard,
            },
            Work::Toggle => InputEvent::ToggleChinese,
            Work::SetChinese(chinese) => {
                if session.backend.with_kernel(|k| k.is_chinese()) == chinese {
                    return Ok(());
                }
                InputEvent::ToggleChinese
            }
            Work::Direct(_) | Work::Finish(_) | Work::Refresh | Work::PairBackspace => {
                return Ok(())
            }
        };
        let input_chinese = session.backend.with_kernel(|k| k.is_chinese());
        let symbol_key = match self.work {
            Work::Key(Key::Char(c), m) if m.is_plain() => Some(c),
            _ => None,
        };
        let symbols_enabled = symbol_key.is_some_and(|c| {
            let r = retype_types::symbols::rules(c, input_chinese);
            r.opening.is_some() || r.closing.is_some()
        }) && crate::preferences::symbol_completion()
            && learnable(&self.context, ec);
        let allow_learning = countable(&self.context);
        let started = Instant::now();
        let actions = session.submit_in_context(event, allow_learning);
        timing.kernel_us += crate::input_profile::micros(started.elapsed());
        if matches!(self.work, Work::Toggle | Work::SetChinese(_)) {
            state.notify_language_bar();
        }
        let mut pass = false;
        let mut committed = Vec::new();
        let mut learning = Vec::new();
        let write_start = Instant::now();
        let write_result: Result<()> = (|| {
            for action in actions {
                match action {
                    KernelAction::Commit(
                        CommitRequest::Text(text) | CommitRequest::ReplaceComposition { text },
                    ) => {
                        crate::symbols::commit(
                            state,
                            &self.context,
                            ec,
                            &text,
                            symbol_key,
                            input_chinese,
                            symbols_enabled,
                        )?;
                        committed.push(text);
                    }
                    KernelAction::Render(render) => {
                        if render.composition.is_empty() {
                            end(state, ec, true)?;
                            state.hide();
                        } else {
                            replace(state, &self.context, ec, &render.composition, true)?;
                        }
                    }
                    KernelAction::PassThrough => pass = true,
                    KernelAction::Side(SideEffect::Learn(event)) => learning.push(event),
                    KernelAction::Side(_) => {}
                }
            }
            // A printable key following a composition must be inserted in the SAME edit
            // transaction as its commit, otherwise an async commit can land after punctuation.
            if pass {
                let literal = match self.work {
                    Work::Key(Key::Char(c), _) => Some(c.to_string()),
                    Work::Key(Key::Space, _) => Some(" ".into()),
                    Work::Key(Key::Enter, _) => Some("\r\n".into()),
                    _ => None,
                };
                if let Some(literal) = literal {
                    end(state, ec, false)?;
                    state.reset_kernel();
                    crate::symbols::commit(
                        state,
                        &self.context,
                        ec,
                        &literal,
                        symbol_key,
                        input_chinese,
                        symbols_enabled,
                    )?;
                    committed.push(literal);
                }
            }
            Ok(())
        })();
        timing.write_us += crate::input_profile::micros(write_start.elapsed());
        write_result?;
        // Only successful host writes count as selections. Failed/cancelled edit sessions
        // and hidden input contexts must not persist a user's uncommitted text.
        if !committed.is_empty() && !learning.is_empty() && learnable(&self.context, ec) {
            for event in learning {
                session.backend.record_learning(event);
            }
        }
        self.refresh(ec, timing)?;
        let discarded = (committed.is_empty()
            || input_chinese && matches!(self.work, Work::Key(Key::Enter, _)))
            && session.backend.with_kernel(|k| !k.has_composition());
        if countable(&self.context)
            && !(input_chinese && matches!(self.work, Work::Key(Key::Enter, _)))
        {
            for text in committed {
                stats::commit(&state.stats_clock, &text, input_chinese);
            }
            if matches!(
                self.work,
                Work::Choose(..)
                    | Work::Toggle
                    | Work::SetChinese(_)
                    | Work::Translate
                    | Work::Boundary(..)
                    | Work::Key(
                        Key::Space
                            | Key::Enter
                            | Key::Tab
                            | Key::Left
                            | Key::Right
                            | Key::Up
                            | Key::Down,
                        _
                    )
            ) {
                stats::boundary(&state.stats_clock);
            }
        }
        if discarded {
            lock(&state.stats_clock).reset();
        }
        if matches!(self.work, Work::Translate | Work::Voice(_)) {
            return if let Work::Voice(held) = self.work {
                crate::voice::capture(state, &self.context, ec, held)
            } else {
                crate::translation::capture(state, &self.context, ec)
            };
        }
        Ok(())
    }
    fn refresh(&self, ec: u32, timing: &mut crate::input_profile::Timing) -> Result<()> {
        let state = &self.state;
        let Some(session) = state.session() else {
            return Ok(());
        };
        let c = lock(&state.composition).clone();
        if let Some(c) = c {
            if c.context != self.context {
                return Ok(());
            }
            // SAFETY: ec is a valid read/write cookie for this composition's context.
            unsafe {
                let start = Instant::now();
                let range = c.object.GetRange()?;
                let mut rect = RECT::default();
                let mut clipped = BOOL(0);
                let anchor = (self
                    .context
                    .GetActiveView()
                    .and_then(|view| view.GetTextExt(ec, &range, &mut rect, &mut clipped))
                    .is_ok()
                    && !clipped.as_bool())
                .then_some(rect);
                timing.anchor_us += crate::input_profile::micros(start.elapsed());
                let initial = session.backend.with_kernel(|k| k.render_state());
                if !matches!(self.work, Work::Refresh)
                    && initial.composition.len() >= 3
                    && session
                        .backend
                        .with_kernel(|k| !k.is_chinese() && k.config().english_spelling)
                {
                    crate::english_updates::watch(state);
                }
                let owner = self
                    .context
                    .GetActiveView()
                    .and_then(|view| view.GetWnd())
                    .ok()
                    .filter(|window| !window.is_invalid())
                    .or_else(|| {
                        let focused = windows::Win32::UI::Input::KeyboardAndMouse::GetFocus();
                        (!focused.is_invalid()).then_some(focused)
                    });
                // Pagination and drawing share these physical pixel measurements.
                let start = Instant::now();
                let layout = crate::popup::measure(&initial, owner, anchor);
                timing.measure_us += crate::input_profile::micros(start.elapsed());
                let start = Instant::now();
                session
                    .backend
                    .layout_candidates(&layout.widths, layout.available, layout.gap);
                let render = session.backend.with_kernel(|k| k.render_state());
                timing.layout_us += crate::input_profile::micros(start.elapsed());
                let manager = lock(&state.thread_mgr).clone();
                let window = lock(&state.window).take();
                if let Some(mut window) = window {
                    if let Some(manager) = manager {
                        let start = Instant::now();
                        match window.update(
                            state,
                            &manager,
                            &self.context,
                            &render,
                            anchor,
                            &layout,
                        ) {
                            Ok(metrics) => {
                                timing.raster_us += metrics.raster_us;
                                timing.notify_us += metrics.notify_us;
                                timing.notify_result = metrics.notify_result;
                                timing.getter_calls += metrics.getter_calls;
                                timing.string_calls += metrics.string_calls;
                                timing.updated_flags |= metrics.updated_flags;
                                timing.reused = metrics.reused;
                                timing.dpi = metrics.dpi;
                            }
                            Err(error) => tracing::warn!("TSF candidate UI update failed: {error}"),
                        }
                        timing.ui_us += crate::input_profile::micros(start.elapsed());
                    }
                    *lock(&state.window) = Some(window);
                }
            }
        }
        Ok(())
    }
}

fn countable(ctx: &ITfContext) -> bool {
    // A host that cannot promise there is no hidden text is not counted.
    // SAFETY: This reads context metadata under its current TSF edit session.
    unsafe { ctx.GetStatus() }.is_ok_and(|status| status.dwStaticFlags & TS_SS_NOHIDDENTEXT != 0)
}

pub(crate) fn learnable(ctx: &ITfContext, ec: u32) -> bool {
    if !countable(ctx) {
        return false;
    }
    // Query scopes only at commit, keeping this extra COM work out of candidate decoding.
    // SAFETY: ec is the host's current granted edit cookie; owned COM values are scoped here.
    unsafe {
        let mut selection = [TF_SELECTION::default()];
        let mut count = 0;
        let selected = ctx.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selection, &mut count);
        let range = ManuallyDrop::take(&mut selection[0].range);
        if selected.is_err() || count == 0 {
            return false;
        }
        let Some(range) = range else {
            return false;
        };
        allowed_input_scope(ctx, ec, &range)
    }
}

/// Explicit translation checks the input scope independently of automatic learning.
/// NOHIDDENTEXT is a host capability flag, not a permission to read normal text.
pub(crate) fn allowed_input_scope(ctx: &ITfContext, ec: u32, range: &ITfRange) -> bool {
    // SAFETY: The caller holds the context's read cookie; range belongs to this context.
    unsafe {
        let property = match ctx.GetAppProperty(&GUID_PROP_INPUTSCOPE) {
            Ok(property) => property,
            Err(error) => return absent_scope(error.code()),
        };
        let value = match property.GetValue(ec, range) {
            Ok(value) => value,
            Err(error) => return absent_scope(error.code()),
        };
        safe_input_scope(&value)
    }
}

fn safe_input_scope(value: &VARIANT) -> bool {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::Variant::VT_EMPTY;
    // SAFETY: reading the VARIANT's discriminator; an absent scope is a normal text field.
    if unsafe { value.Anonymous.Anonymous.vt } == VT_EMPTY {
        return true;
    }
    let Ok(unknown) = windows_core::IUnknown::try_from(value) else {
        return false;
    };
    let Ok(scope) = unknown.cast::<ITfInputScope>() else {
        return false;
    };
    let mut scopes = std::ptr::null_mut();
    let mut count = 0;
    // SAFETY: COM allocates this array and provides its length. Always free it, including
    // failure paths, and reject unexpectedly large counts before constructing a slice.
    unsafe {
        let result = scope.GetInputScopes(&mut scopes, &mut count);
        let allowed = result.is_ok()
            && count <= 64
            && (count == 0 || !scopes.is_null())
            && (count == 0
                || !std::slice::from_raw_parts(scopes, count as usize)
                    .iter()
                    .any(|scope| matches!(*scope, IS_PASSWORD | IS_PRIVATE | IS_NUMERIC_PIN)));
        CoTaskMemFree(Some(scopes.cast()));
        allowed
    }
}

fn absent_scope(status: windows_core::HRESULT) -> bool {
    // Legacy ITextStoreACP hosts without an InputScope attribute report E_FAIL or
    // E_NOTIMPL for this optional app property.
    matches!(
        status,
        windows::Win32::Foundation::E_FAIL | windows::Win32::Foundation::E_NOTIMPL
    )
}

pub(crate) fn replace(
    state: &Arc<TipState>,
    ctx: &ITfContext,
    ec: u32,
    text: &str,
    composing: bool,
) -> Result<()> {
    let current = lock(&state.composition).clone();
    let utf16: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: ec grants write access to ctx; all ranges and interfaces belong to ctx.
    unsafe {
        let range = if let Some(c) = current {
            let range = c.object.GetRange()?;
            range.SetText(ec, 0, &utf16)?;
            range
        } else {
            let insert: ITfInsertAtSelection = ctx.cast()?;
            // Query the selected range without changing the document first.
            let range = insert.InsertTextAtSelection(ec, TF_IAS_QUERYONLY, &[])?;
            if composing {
                let source: ITfSource = ctx.cast()?;
                let observer: ITfTextEditSink = Observer {
                    state: Arc::downgrade(state),
                }
                .into();
                let cookie = source.AdviseSink(&ITfTextEditSink::IID, &observer)?;
                let layout: ITfTextLayoutSink = observer.cast()?;
                let layout_cookie = match source.AdviseSink(&ITfTextLayoutSink::IID, &layout) {
                    Ok(cookie) => cookie,
                    Err(error) => {
                        let _ = source.UnadviseSink(cookie);
                        return Err(error);
                    }
                };
                let manager: ITfContextComposition = ctx.cast()?;
                let sink: ITfCompositionSink = CompositionSink {
                    state: Arc::downgrade(state),
                }
                .into();
                let object = match manager.StartComposition(ec, &range, &sink) {
                    Ok(object) => object,
                    Err(e) => {
                        let _ = source.UnadviseSink(cookie);
                        let _ = source.UnadviseSink(layout_cookie);
                        return Err(e);
                    }
                };
                *lock(&state.composition) = Some(Composition {
                    context: ctx.clone(),
                    object,
                    source,
                    cookie,
                    layout_cookie,
                });
            }
            range.SetText(ec, 0, &utf16)?;
            range
        };
        if composing {
            if let Ok(property) = ctx.GetProperty(&GUID_PROP_ATTRIBUTE) {
                let value = VARIANT::from(state.attribute.load(Ordering::SeqCst) as i32);
                let _ = property.SetValue(ec, &range, &value);
            }
        }
        let caret = range.Clone()?;
        caret.Collapse(ec, TF_ANCHOR_END)?;
        let mut selection = TF_SELECTION {
            range: ManuallyDrop::new(Some(caret)),
            style: TF_SELECTIONSTYLE {
                ase: TF_AE_NONE,
                fInterimChar: BOOL(0),
            },
        };
        let result = ctx.SetSelection(ec, std::slice::from_ref(&selection));
        ManuallyDrop::drop(&mut selection.range);
        result?;
        if !composing {
            end(state, ec, false)?;
        }
    }
    Ok(())
}

fn end(state: &TipState, ec: u32, cancel: bool) -> Result<()> {
    let current = lock(&state.composition).take();
    if let Some(c) = current {
        // SAFETY: Caller holds a write lock for c.context; detach observer before our edits.
        unsafe {
            let _ = c.source.UnadviseSink(c.cookie);
            let _ = c.source.UnadviseSink(c.layout_cookie);
            let range = c.object.GetRange()?;
            if let Ok(property) = c.context.GetProperty(&GUID_PROP_ATTRIBUTE) {
                let _ = property.Clear(ec, &range);
            }
            let edit = if cancel {
                range.SetText(ec, 0, &[])
            } else {
                Ok(())
            };
            let ended = c.object.EndComposition(ec);
            edit.and(ended)?;
        }
    }
    Ok(())
}

#[implement(ITfCompositionSink)]
struct CompositionSink {
    state: Weak<TipState>,
}
impl ITfCompositionSink_Impl for CompositionSink_Impl {
    fn OnCompositionTerminated(&self, ec: u32, composition: Ref<'_, ITfComposition>) -> Result<()> {
        guarded(|| {
            if let Some(state) = self.state.upgrade() {
                let current = lock(&state.composition).clone();
                if current
                    .as_ref()
                    .is_some_and(|c| composition.ok().is_ok_and(|object| c.object == *object))
                {
                    let current = lock(&state.composition).take();
                    if let Some(c) = current {
                        // SAFETY: TSF provides ec with write access during termination.
                        unsafe {
                            let _ = c.source.UnadviseSink(c.cookie);
                            let _ = c.source.UnadviseSink(c.layout_cookie);
                            if let (Ok(property), Ok(range)) = (
                                c.context.GetProperty(&GUID_PROP_ATTRIBUTE),
                                c.object.GetRange(),
                            ) {
                                let _ = property.Clear(ec, &range);
                            }
                        }
                    }
                    state.epoch.fetch_add(1, Ordering::SeqCst);
                    state.reset_kernel();
                    state.hide();
                }
            }
            Ok(())
        })
    }
}
#[implement(ITfTextEditSink, ITfTextLayoutSink)]
struct Observer {
    state: Weak<TipState>,
}
impl ITfTextLayoutSink_Impl for Observer_Impl {
    fn OnLayoutChange(
        &self,
        ctx: Ref<'_, ITfContext>,
        code: TfLayoutCode,
        _view: Ref<'_, ITfContextView>,
    ) -> Result<()> {
        guarded(|| {
            if let Some(state) = self.state.upgrade() {
                if code == TF_LC_DESTROY {
                    state.epoch.fetch_add(1, Ordering::SeqCst);
                    state.finish(false);
                } else if !state.writing.load(Ordering::SeqCst) {
                    let _ = request(&state, ctx.ok()?, Work::Refresh);
                }
            }
            Ok(())
        })
    }
}
impl ITfTextEditSink_Impl for Observer_Impl {
    fn OnEndEdit(
        &self,
        ctx: Ref<'_, ITfContext>,
        ec: u32,
        record: Ref<'_, ITfEditRecord>,
    ) -> Result<()> {
        guarded(|| {
            let Some(state) = self.state.upgrade() else {
                return Ok(());
            };
            if state.writing.load(Ordering::SeqCst) {
                return Ok(());
            }
            let Some(c) = lock(&state.composition).clone() else {
                return Ok(());
            };
            // SAFETY: ec permits reads; any necessary write is requested as a later edit session.
            unsafe {
                if record.ok()?.GetSelectionStatus()?.as_bool() {
                    let mut selection = [TF_SELECTION::default()];
                    let mut count = 0;
                    ctx.ok()?
                        .GetSelection(ec, TF_DEFAULT_SELECTION, &mut selection, &mut count)?;
                    let selected = ManuallyDrop::take(&mut selection[0].range);
                    if let Some(selected) = selected {
                        let range = c.object.GetRange()?;
                        if selected.CompareStart(ec, &range, TF_ANCHOR_START)? < 0
                            || selected.CompareEnd(ec, &range, TF_ANCHOR_END)? > 0
                        {
                            state.epoch.fetch_add(1, Ordering::SeqCst);
                            state.finish(false);
                        }
                    }
                }
            }
            Ok(())
        })
    }
}
