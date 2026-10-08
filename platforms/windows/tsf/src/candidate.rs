//! TSF candidate discovery plus a no-activate native popup for desktop hosts.
use retype_types::RenderState;
use std::sync::{
    atomic::{AtomicU32, AtomicU64, Ordering},
    Arc, Mutex, MutexGuard, Weak,
};
use std::time::Instant;
use windows::Win32::Foundation::{E_INVALIDARG, E_POINTER, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::TextServices::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::{implement, Interface, Result, BOOL, BSTR, GUID};

#[derive(Default)]
pub struct CandidateWindow {
    window: Option<HWND>,
    ui: Option<(ITfUIElementMgr, u32)>,
    element: Option<ITfCandidateListUIElementBehavior>,
    data: Arc<Mutex<RenderState>>,
    visibility: Arc<Mutex<Visibility>>,
    placement: Option<(i32, i32, i32, i32)>,
    selection: Arc<Mutex<Option<(u64, usize)>>>,
    updated_flags: Arc<AtomicU32>,
    reads: Arc<CandidateReads>,
    context: Option<ITfContext>,
    pending_notification: u32,
}

#[derive(Default)]
struct CandidateReads {
    getters: AtomicU64,
    strings: AtomicU64,
}

const ALL_UPDATED: u32 = TF_CLUIE_COUNT
    | TF_CLUIE_SELECTION
    | TF_CLUIE_STRING
    | TF_CLUIE_PAGEINDEX
    | TF_CLUIE_CURRENTPAGE
    | TF_CLUIE_DOCUMENTMGR;

fn selected(state: &RenderState, selection: Option<(u64, usize)>) -> usize {
    selection
        .filter(|(generation, index)| *generation == state.gen && *index < state.candidates.len())
        .map_or(state.selected, |(_, index)| index)
}

fn current_page(state: &RenderState) -> usize {
    if state.page_starts.is_empty() {
        state.page_start / state.page_size.max(1)
    } else {
        state
            .page_starts
            .partition_point(|&start| start <= state.page_start)
            .saturating_sub(1)
    }
}

fn page_start(state: &RenderState, page: usize) -> usize {
    state
        .page_starts
        .get(page)
        .copied()
        .unwrap_or(page * state.page_size.max(1))
}

// Only compare fields exposed by the candidate-list interface. Generation and
// consumption metadata still get published, without asking hosts to reread text.
fn updated_flags(old: &RenderState, new: &RenderState, selection: Option<(u64, usize)>) -> u32 {
    let mut flags = 0;
    if old.candidates.len() != new.candidates.len() {
        flags |= TF_CLUIE_COUNT;
    }
    if !old
        .candidates
        .iter()
        .map(|c| c.text.as_str())
        .eq(new.candidates.iter().map(|c| c.text.as_str()))
    {
        flags |= TF_CLUIE_STRING;
    }
    if selected(old, selection) != selected(new, selection) {
        flags |= TF_CLUIE_SELECTION;
    }
    if old.page_count() != new.page_count()
        || (0..old.page_count()).any(|i| page_start(old, i) != page_start(new, i))
    {
        flags |= TF_CLUIE_PAGEINDEX;
    }
    if current_page(old) != current_page(new) {
        flags |= TF_CLUIE_CURRENTPAGE;
    }
    flags
}

#[derive(Default)]
struct Visibility {
    window: Option<HWND>,
    requested: bool,
}

impl CandidateWindow {
    #[cfg(test)]
    pub(crate) fn native_handle(&self) -> Option<HWND> {
        self.window
    }

    #[cfg(test)]
    pub(crate) fn element_for_test(&self) -> Option<ITfCandidateListUIElementBehavior> {
        self.element.clone()
    }

    pub fn hide(&mut self) {
        // SAFETY: The popup is owned and accessed by this TSF apartment only.
        unsafe {
            *self.visibility.lock().unwrap_or_else(|e| e.into_inner()) = Visibility::default();
            if let Some(window) = self.window.take() {
                let _ = DestroyWindow(window);
            }
            if let Some((mgr, id)) = self.ui.take() {
                let _ = mgr.EndUIElement(id);
            }
        }
        self.element = None;
        self.placement = None;
        self.context = None;
        self.pending_notification = 0;
    }
    #[allow(clippy::arc_with_non_send_sync)] // Shared only by COM objects in this STA.
    pub fn update(
        &mut self,
        tip: &Arc<crate::tip::TipState>,
        mgr: &ITfThreadMgr,
        ctx: &ITfContext,
        state: &RenderState,
        anchor: Option<RECT>,
        layout: &crate::popup::Layout,
    ) -> Result<crate::popup::UpdateMetrics> {
        if self.context.as_ref().is_some_and(|bound| bound != ctx) {
            self.hide();
        }
        let new_element = self.element.is_none();
        if new_element {
            // A retained element must keep its own data and counters after EndUIElement.
            self.data = Arc::default();
            self.selection = Arc::default();
            self.updated_flags = Arc::default();
            self.reads = Arc::default();
        }
        let flags = {
            let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
            let selection = *self.selection.lock().unwrap_or_else(|e| e.into_inner());
            let flags = if new_element {
                ALL_UPDATED
            } else {
                updated_flags(&data, state, selection)
            };
            let changed = *data != *state;
            if changed {
                *data = state.clone();
            }
            flags
        } | self.pending_notification;
        // Keep the last notification's flags available to hosts which query
        // after UpdateUIElement returns; a caret-only refresh must not erase it.
        if flags != 0 {
            self.updated_flags.store(flags, Ordering::Relaxed);
        }
        let mut metrics = crate::popup::UpdateMetrics {
            updated_flags: flags,
            ..Default::default()
        };
        if state.composition.is_empty() || state.candidates.is_empty() {
            self.hide();
            return Ok(Default::default());
        }
        // SAFETY: COM objects and HWND belong to the calling TSF UI thread. No global hooks.
        unsafe {
            let getters_before = self.reads.getters.load(Ordering::Relaxed);
            let strings_before = self.reads.strings.load(Ordering::Relaxed);
            if self.element.is_none() {
                self.visibility = Arc::new(Mutex::new(Visibility {
                    window: self.window,
                    requested: true,
                }));
                let element: ITfCandidateListUIElementBehavior = Candidates {
                    data: Arc::clone(&self.data),
                    document: ctx.GetDocumentMgr()?,
                    context: ctx.clone(),
                    tip: Arc::downgrade(tip),
                    selection: Arc::clone(&self.selection),
                    updated_flags: Arc::clone(&self.updated_flags),
                    reads: Arc::clone(&self.reads),
                    visibility: Arc::clone(&self.visibility),
                }
                .into();
                self.visibility
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .requested = true;
                let ui_mgr: ITfUIElementMgr = mgr.cast()?;
                let mut show = BOOL(1);
                let mut id = 0;
                let ui: ITfUIElement = element.cast()?;
                let start = Instant::now();
                ui_mgr.BeginUIElement(&ui, &mut show, &mut id)?;
                metrics.notify_us += crate::input_profile::micros(start.elapsed());
                self.visibility
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .requested = show.as_bool();
                self.ui = Some((ui_mgr, id));
                self.element = Some(element);
                self.context = Some(ctx.clone());
            }
            if let Some((ui_mgr, id)) = self.ui.as_ref().filter(|_| flags != 0) {
                let start = Instant::now();
                match ui_mgr.UpdateUIElement(*id) {
                    Ok(()) => self.pending_notification = 0,
                    Err(error) => {
                        self.pending_notification = flags;
                        metrics.notify_result = error.code().0;
                    }
                }
                metrics.notify_us += crate::input_profile::micros(start.elapsed());
            }
            metrics.getter_calls = self
                .reads
                .getters
                .load(Ordering::Relaxed)
                .wrapping_sub(getters_before);
            metrics.string_calls = self
                .reads
                .strings
                .load(Ordering::Relaxed)
                .wrapping_sub(strings_before);
            // A UI-less host renders the candidate list itself. Do not create a
            // popup inside that host, even if it accepts our UI element updates.
            if !self
                .visibility
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .requested
            {
                if let Some(window) = self.window {
                    let _ = ShowWindow(window, SW_HIDE);
                }
                return Ok(metrics);
            }
            // UI-less controls can expose a composition without a screen-space
            // text extent. They still need Begin/UpdateUIElement above so the
            // host receives candidates; only our own popup requires an anchor.
            let Some(anchor) = anchor else {
                if let Some(window) = self.window {
                    let _ = ShowWindow(window, SW_HIDE);
                }
                return Ok(metrics);
            };
            let owner = ctx
                .GetActiveView()
                .and_then(|view| view.GetWnd())
                .ok()
                .filter(|window| !window.is_invalid())
                .or_else(|| {
                    let focused = windows::Win32::UI::Input::KeyboardAndMouse::GetFocus();
                    (!focused.is_invalid()).then_some(focused)
                });
            if self
                .window
                .is_some_and(|window| !IsWindow(Some(window)).as_bool())
            {
                self.window = None;
                self.placement = None;
            }
            if self.window.is_none() {
                self.window = Some(crate::popup::create(owner)?);
                self.visibility
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .window = self.window;
            }
            if let Some(window) = self.window {
                // Clamp to the work area on the caret's monitor (including negative coordinates).
                let monitor = windows::Win32::Graphics::Gdi::MonitorFromRect(
                    &anchor,
                    windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST,
                );
                let mut info = windows::Win32::Graphics::Gdi::MONITORINFO {
                    cbSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::MONITORINFO>()
                        as u32,
                    ..Default::default()
                };
                let has_monitor =
                    windows::Win32::Graphics::Gdi::GetMonitorInfoW(monitor, &mut info).as_bool();
                let update = crate::popup::update(window, tip, ctx, state, layout);
                let (width, height) = (update.width, update.height);
                let (mut x, mut y) = (anchor.left, anchor.bottom + 4);
                if has_monitor {
                    x = x
                        .max(info.rcWork.left)
                        .min((info.rcWork.right - width).max(info.rcWork.left));
                    if y + height > info.rcWork.bottom {
                        y = anchor.top - height - 4;
                    }
                    y = y.max(info.rcWork.top);
                }
                let placement = (x, y, width, height);
                if self.placement != Some(placement) {
                    SetWindowPos(
                        window,
                        Some(HWND_TOPMOST),
                        x,
                        y,
                        width,
                        height,
                        SWP_NOACTIVATE,
                    )?;
                    self.placement = Some(placement);
                }
                let show = self
                    .visibility
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .requested;
                if IsWindowVisible(window).as_bool() != show {
                    let _ = ShowWindow(window, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
                }
                metrics.raster_us = update.metrics.raster_us;
                metrics.reused = update.metrics.reused;
                metrics.dpi = update.metrics.dpi;
                return Ok(metrics);
            }
        }
        Ok(metrics)
    }
}
impl Drop for CandidateWindow {
    fn drop(&mut self) {
        self.hide();
        self.visibility
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .window = None;
        if let Some(window) = self.window.take() {
            // SAFETY: Only the creating thread owns this window.
            let _ = unsafe { DestroyWindow(window) };
        }
    }
}

#[implement(
    ITfCandidateListUIElementBehavior,
    ITfIntegratableCandidateListUIElement
)]
struct Candidates {
    data: Arc<Mutex<RenderState>>,
    document: ITfDocumentMgr,
    context: ITfContext,
    tip: Weak<crate::tip::TipState>,
    selection: Arc<Mutex<Option<(u64, usize)>>>,
    updated_flags: Arc<AtomicU32>,
    reads: Arc<CandidateReads>,
    visibility: Arc<Mutex<Visibility>>,
}
impl Candidates_Impl {
    fn state(&self) -> MutexGuard<'_, RenderState> {
        self.reads.getters.fetch_add(1, Ordering::Relaxed);
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn selected(&self, state: &RenderState) -> usize {
        selected(
            state,
            *self.selection.lock().unwrap_or_else(|e| e.into_inner()),
        )
    }

    fn request(&self, work: crate::edit::Work) -> Result<()> {
        let tip = self
            .tip
            .upgrade()
            .ok_or(windows::Win32::Foundation::E_FAIL)?;
        crate::edit::request(&tip, &self.context, work)
    }
}
impl ITfUIElement_Impl for Candidates_Impl {
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("retype candidates"))
    }
    fn GetGUID(&self) -> Result<GUID> {
        Ok(crate::ids::GUID_PROFILE_RETYPE)
    }
    fn Show(&self, show: BOOL) -> Result<()> {
        let window = {
            let mut visibility = self.visibility.lock().unwrap_or_else(|e| e.into_inner());
            visibility.requested = show.as_bool();
            visibility.window
        };
        if let Some(window) = window {
            // SAFETY: TSF calls this interface on the owning apartment.
            unsafe {
                let _ = ShowWindow(
                    window,
                    if show.as_bool() {
                        SW_SHOWNOACTIVATE
                    } else {
                        SW_HIDE
                    },
                );
            }
        }
        Ok(())
    }
    fn IsShown(&self) -> Result<BOOL> {
        let window = self
            .visibility
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .window;
        // SAFETY: Query only; a retained UI element has its handle cleared on destruction.
        Ok(window
            .is_some_and(|window| unsafe { IsWindowVisible(window).as_bool() })
            .into())
    }
}
impl ITfCandidateListUIElement_Impl for Candidates_Impl {
    fn GetUpdatedFlags(&self) -> Result<u32> {
        self.reads.getters.fetch_add(1, Ordering::Relaxed);
        Ok(self.updated_flags.load(Ordering::Relaxed))
    }
    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        self.reads.getters.fetch_add(1, Ordering::Relaxed);
        Ok(self.document.clone())
    }
    fn GetCount(&self) -> Result<u32> {
        Ok(self.state().candidates.len() as u32)
    }
    fn GetSelection(&self) -> Result<u32> {
        let state = self.state();
        Ok(self.selected(&state) as u32)
    }
    fn GetString(&self, index: u32) -> Result<BSTR> {
        self.reads.strings.fetch_add(1, Ordering::Relaxed);
        self.state()
            .candidates
            .get(index as usize)
            .map(|c| BSTR::from(c.text.as_str()))
            .ok_or_else(|| E_INVALIDARG.into())
    }
    fn GetPageIndex(&self, indexes: *mut u32, size: u32, count: *mut u32) -> Result<()> {
        if count.is_null() || (size > 0 && indexes.is_null()) {
            return Err(E_POINTER.into());
        }
        let state = self.state();
        // SAFETY: Caller supplies size writable elements and a count out-parameter.
        unsafe {
            *count = state.page_count() as u32;
            for i in 0..(size as usize).min(state.page_count()) {
                *indexes.add(i) = state
                    .page_starts
                    .get(i)
                    .copied()
                    .unwrap_or(i * state.page_size.max(1)) as u32;
            }
        }
        if (size as usize) < state.page_count() {
            return Err(windows_core::HRESULT(1).into());
        }
        Ok(())
    }
    fn SetPageIndex(&self, _indexes: *const u32, _count: u32) -> Result<()> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }
    fn GetCurrentPage(&self) -> Result<u32> {
        Ok(current_page(&self.state()) as u32)
    }
}

impl ITfCandidateListUIElementBehavior_Impl for Candidates_Impl {
    fn SetSelection(&self, index: u32) -> Result<()> {
        let state = self.state();
        if index as usize >= state.candidates.len() {
            return Err(E_INVALIDARG.into());
        }
        *self.selection.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((state.gen, index as usize));
        Ok(())
    }

    fn Finalize(&self) -> Result<()> {
        let (index, generation) = {
            let state = self.state();
            (self.selected(&state), state.gen)
        };
        // Never hold candidate data across a reentrant host edit request.
        self.request(crate::edit::Work::Choose(index, generation))
    }

    fn Abort(&self) -> Result<()> {
        self.request(crate::edit::Work::Finish(true))
    }
}

impl ITfIntegratableCandidateListUIElement_Impl for Candidates_Impl {
    fn SetIntegrationStyle(&self, _style: &GUID) -> Result<()> {
        Ok(())
    }

    fn GetSelectionStyle(&self) -> Result<TfIntegratableCandidateListSelectionStyle> {
        Ok(STYLE_ACTIVE_SELECTION)
    }

    fn OnKeyDown(&self, key: WPARAM, flags: LPARAM) -> Result<BOOL> {
        let Some(tip) = self.tip.upgrade() else {
            return Ok(BOOL(0));
        };
        let scan = (flags.0 as u32 >> 16) & 0xff;
        let Some(key) = crate::keymap::translate_event(key.0 as u16, scan) else {
            return Ok(BOOL(0));
        };
        let modifiers = crate::keymap::read_modifiers();
        if !tip.wants(key, modifiers) {
            return Ok(BOOL(0));
        }
        crate::edit::request(&tip, &self.context, crate::edit::Work::Key(key, modifiers))?;
        Ok(BOOL(1))
    }

    fn ShowCandidateNumbers(&self) -> Result<BOOL> {
        Ok(BOOL(1))
    }

    fn FinalizeExactCompositionString(&self) -> Result<()> {
        self.request(crate::edit::Work::Key(
            retype_types::Key::Enter,
            retype_types::Modifiers::NONE,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use retype_types::{Candidate, CandidateSource};

    #[test]
    fn flags_track_exposed_text_count_selection_and_variable_pages() {
        let old = RenderState {
            candidates: (0..20)
                .map(|i| Candidate::new(i.to_string(), CandidateSource::Local))
                .collect(),
            page_size: 8,
            page_starts: vec![0, 8, 13],
            ..Default::default()
        };
        let mut new = old.clone();
        new.gen += 1;
        new.composition = "different internal composition".into();
        new.candidates[0].score = 1.0;
        new.candidates[0].consumed = 2;
        assert_eq!(updated_flags(&old, &new, None), 0);
        new.candidates[19].text = "changed".into();
        assert_eq!(updated_flags(&old, &new, None), TF_CLUIE_STRING);
        new = old.clone();
        new.page_starts = vec![0, 7, 12];
        assert_eq!(updated_flags(&old, &new, None), TF_CLUIE_PAGEINDEX);
        new = old.clone();
        new.selected = 13;
        new.page_start = 13;
        assert_eq!(
            updated_flags(&old, &new, None),
            TF_CLUIE_SELECTION | TF_CLUIE_CURRENTPAGE
        );
        new = old.clone();
        new.candidates.pop();
        assert_eq!(
            updated_flags(&old, &new, None),
            TF_CLUIE_COUNT | TF_CLUIE_STRING
        );
        // Equivalent fixed and explicit boundaries must not cause a reread.
        let mut fixed = old.clone();
        fixed.page_starts.clear();
        let mut explicit = fixed.clone();
        explicit.page_starts = vec![0, 8, 16];
        assert_eq!(updated_flags(&fixed, &explicit, None), 0);
    }
}
