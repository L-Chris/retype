//! Composition transactions. Never write a host document outside DoEditSession.
use crate::stats::{self, Language};
use crate::tip::{guarded, lock, TipState};

use retype_types::{CommitRequest, InputEvent, InputSource, KernelAction, Key, Modifiers};
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
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
    Direct(char),
    Toggle,
    SetChinese(bool),
    Choose(usize, u64),
    Finish(bool),
    Refresh,
}

pub(crate) fn request(state: &Arc<TipState>, context: &ITfContext, work: Work) -> Result<()> {
    let pending_counted = !matches!(work, Work::Direct(_));
    let cancelled = Arc::new(AtomicBool::new(false));
    let completed = Arc::new(AtomicBool::new(false));
    let session: ITfEditSession = Edit {
        state: Arc::clone(state),
        context: context.clone(),
        work,
        epoch: state.epoch.load(Ordering::SeqCst),
        finishing: if matches!(work, Work::Finish(_)) {
            lock(&state.composition).as_ref().map(|c| c.object.clone())
        } else {
            None
        },
        cancelled: Arc::clone(&cancelled),
        completed: Arc::clone(&completed),
        pending_counted,
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
            if matches!(work, Work::Direct(_)) {
                TF_ES_SYNC
            } else {
                TF_ES_ASYNCDONTCARE
            } | if matches!(work, Work::Refresh) {
                TF_ES_READ
            } else {
                TF_ES_READWRITE
            },
        )?;
        if matches!(work, Work::Direct(_)) && !completed.load(Ordering::SeqCst) {
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
}
impl Drop for Edit {
    fn drop(&mut self) {
        if self.pending_counted {
            self.state.pending.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
impl ITfEditSession_Impl for Edit_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        guarded(|| {
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
                return self.refresh(ec);
            }
            self.state.writing.store(true, Ordering::SeqCst);
            let result = guarded(|| self.run(ec));
            self.state.writing.store(false, Ordering::SeqCst);
            if result.is_ok() && matches!(self.work, Work::Direct(_)) {
                self.completed.store(true, Ordering::SeqCst);
            }
            if result.is_err() {
                // Fail closed: leave existing document text intact, release composition ownership.
                let current = lock(&self.state.composition).clone();
                if current.as_ref().is_some_and(|c| c.context == self.context) {
                    let _ = end(&self.state, ec, false);
                }
                self.state.reset_kernel();
                self.state.hide();
            }
            result
        })
    }
}
impl Edit_Impl {
    fn run(&self, ec: u32) -> Result<()> {
        let state = &self.state;
        if matches!(self.work, Work::Refresh) {
            return self.refresh(ec);
        }
        if let Work::Finish(cancel) = self.work {
            let c = lock(&state.composition).clone();
            if c.as_ref().is_some_and(|c| {
                c.context == self.context && self.finishing.as_ref() == Some(&c.object)
            }) {
                end(state, ec, cancel)?;
                state.reset_kernel();
            }
            return Ok(());
        }
        if let Work::Direct(ch) = self.work {
            let text = ch.to_string();
            replace(state, &self.context, ec, &text, false)?;
            if countable(&self.context) {
                stats::activity(&state.stats_clock, Language::English);
                stats::commit(&text);
            }
            return Ok(());
        }
        let Some(session) = state.session() else {
            return Ok(());
        };
        let existing = lock(&state.composition).clone();
        if existing.as_ref().is_some_and(|c| c.context != self.context) {
            // Cannot edit an old context with a cookie issued for a different document.
            state.finish(false);
            return Err(E_FAIL.into());
        }
        let event = match self.work {
            Work::Choose(index, generation) => {
                if session.backend.with_kernel(|k| k.generation()) != generation {
                    return Ok(());
                }
                InputEvent::CandidateChosen { index }
            }
            Work::Key(key, mods) => InputEvent::Key {
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
            Work::Direct(_) | Work::Finish(_) | Work::Refresh => return Ok(()),
        };
        let actions = session.submit(event);
        if matches!(self.work, Work::Toggle | Work::SetChinese(_)) {
            state.notify_language_bar();
        }
        let mut pass = false;
        let mut committed = Vec::new();
        for action in actions {
            match action {
                KernelAction::Commit(
                    CommitRequest::Text(text) | CommitRequest::ReplaceComposition { text },
                ) => {
                    replace(state, &self.context, ec, &text, false)?;
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
                replace(state, &self.context, ec, &literal, false)?;
                committed.push(literal);
            }
        }
        self.refresh(ec)?;
        if countable(&self.context) && !matches!(self.work, Work::Key(Key::Enter, _)) {
            for text in committed {
                stats::commit(&text);
            }
        }
        Ok(())
    }
    fn refresh(&self, ec: u32) -> Result<()> {
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
                let initial = session.backend.with_kernel(|k| k.render_state());
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
                let layout = crate::popup::measure(&initial, owner, anchor);
                session
                    .backend
                    .layout_candidates(&layout.widths, layout.available, layout.gap);
                let render = session.backend.with_kernel(|k| k.render_state());
                let manager = lock(&state.thread_mgr).clone();
                let window = lock(&state.window).take();
                if let Some(mut window) = window {
                    if let Some(manager) = manager {
                        if let Err(error) =
                            window.update(state, &manager, &self.context, &render, anchor, &layout)
                        {
                            tracing::warn!("TSF candidate UI update failed: {error}");
                        }
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

fn replace(
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
