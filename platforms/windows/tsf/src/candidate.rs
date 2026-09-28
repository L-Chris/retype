//! TSF candidate discovery plus a no-activate native popup for desktop hosts.
use retype_types::RenderState;
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{E_INVALIDARG, E_POINTER, HWND, RECT};
use windows::Win32::UI::TextServices::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::{implement, Interface, Result, BOOL, BSTR, GUID};

#[derive(Default)]
pub struct CandidateWindow {
    window: Option<HWND>,
    ui: Option<(ITfUIElementMgr, u32)>,
    element: Option<ITfCandidateListUIElement>,
    data: Arc<Mutex<RenderState>>,
    visibility: Arc<Mutex<Visibility>>,
}

#[derive(Default)]
struct Visibility {
    window: Option<HWND>,
    requested: bool,
}

impl CandidateWindow {
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
    }
    #[allow(clippy::arc_with_non_send_sync)] // Shared only by COM objects in this STA.
    pub fn update(
        &mut self,
        tip: &Arc<crate::tip::TipState>,
        mgr: &ITfThreadMgr,
        ctx: &ITfContext,
        state: &RenderState,
        anchor: RECT,
    ) -> Result<()> {
        *self.data.lock().unwrap_or_else(|e| e.into_inner()) = state.clone();
        if state.composition.is_empty() || state.candidates.is_empty() {
            self.hide();
            return Ok(());
        }
        // SAFETY: COM objects and HWND belong to the calling TSF UI thread. No global hooks.
        unsafe {
            if self.element.is_none() {
                self.visibility = Arc::new(Mutex::new(Visibility {
                    window: self.window,
                    requested: true,
                }));
                let element: ITfCandidateListUIElement = Candidates {
                    data: Arc::clone(&self.data),
                    document: ctx.GetDocumentMgr()?,
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
                ui_mgr.BeginUIElement(&ui, &mut show, &mut id)?;
                self.visibility
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .requested = show.as_bool();
                self.ui = Some((ui_mgr, id));
                self.element = Some(element);
            }
            if let Some((ui_mgr, id)) = &self.ui {
                let _ = ui_mgr.UpdateUIElement(*id);
            }
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
                return Ok(());
            }
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
                let max_width = if has_monitor {
                    info.rcWork.right - info.rcWork.left - 16
                } else {
                    960
                };
                let (width, height) =
                    crate::popup::update(window, tip, ctx, state, max_width.max(100));
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
                SetWindowPos(
                    window,
                    Some(HWND_TOPMOST),
                    x,
                    y,
                    width,
                    height,
                    SWP_NOACTIVATE,
                )?;
                let show = self
                    .visibility
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .requested;
                let _ = ShowWindow(window, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
        Ok(())
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

#[implement(ITfCandidateListUIElement)]
struct Candidates {
    data: Arc<Mutex<RenderState>>,
    document: ITfDocumentMgr,
    visibility: Arc<Mutex<Visibility>>,
}
impl Candidates_Impl {
    fn state(&self) -> RenderState {
        self.data.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
        Ok(TF_CLUIE_COUNT
            | TF_CLUIE_SELECTION
            | TF_CLUIE_STRING
            | TF_CLUIE_PAGEINDEX
            | TF_CLUIE_CURRENTPAGE
            | TF_CLUIE_DOCUMENTMGR)
    }
    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        Ok(self.document.clone())
    }
    fn GetCount(&self) -> Result<u32> {
        Ok(self.state().candidates.len() as u32)
    }
    fn GetSelection(&self) -> Result<u32> {
        Ok(self.state().selected as u32)
    }
    fn GetString(&self, index: u32) -> Result<BSTR> {
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
                *indexes.add(i) = (i * state.page_size.max(1)) as u32;
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
        let s = self.state();
        Ok((s.page_start / s.page_size.max(1)) as u32)
    }
}
