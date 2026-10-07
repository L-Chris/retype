//! Voice preview lives on the TSF apartment; audio/network stay in the helper.
use crate::tip::{guarded, lock, TipState};
use retype_ai::voice::{Command, Phase, Snapshot};
use std::{
    mem::ManuallyDrop,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Weak,
    },
    time::{Duration, Instant},
};
use windows::{
    core::{implement, w, Result, PCWSTR},
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::*,
        UI::{
            HiDpi::GetDpiForWindow, Input::KeyboardAndMouse::GetAsyncKeyState, TextServices::*,
            WindowsAndMessaging::*,
        },
    },
};
struct Frame {
    state: Weak<TipState>,
    context: ITfContext,
    range: ITfRange,
    original: Vec<u16>,
    epoch: u32,
    owner: HWND,
    commands: mpsc::Sender<bool>,
    updates: mpsc::Receiver<std::result::Result<Snapshot, String>>,
    cancelled: Arc<AtomicBool>,
    snapshot: Snapshot,
    held: Option<u16>,
    finishing: bool,
    applying: bool,
    deadline: Instant,
    hide_at: Option<Instant>,
    scale: i32,
    scroll: i32,
    max_scroll: i32,
    follow_tail: bool,
}
impl Drop for Frame {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.commands.send(false);
    }
}
pub(crate) fn dismiss(state: &TipState) {
    if let Some(window) = lock(&state.voice_window).take() {
        unsafe {
            let _ = DestroyWindow(HWND(window as *mut _));
        }
    }
}
pub(crate) fn active(state: &TipState) -> bool {
    lock(&state.voice_window).is_some()
}
pub(crate) fn release_hold(state: &TipState) {
    let window = *lock(&state.voice_window);
    if let Some(window) = window {
        // The key callback and this window belong to the same TSF apartment.
        unsafe {
            if let Some(f) = frame(HWND(window as *mut _)) {
                if f.held == Some(retype_ai::config::Shortcut::VOICE.vk) {
                    finish(f);
                }
            }
        }
    }
}
pub(crate) fn capture(
    state: &Arc<TipState>,
    context: &ITfContext,
    ec: u32,
    held: bool,
) -> Result<()> {
    if active(state) {
        return Ok(());
    }
    unsafe {
        if context.GetStatus()?.dwDynamicFlags & TS_SD_READONLY != 0 {
            return Err(E_ACCESSDENIED.into());
        }
        let mut selection = [TF_SELECTION::default()];
        let mut fetched = 0;
        context.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selection, &mut fetched)?;
        let range = ManuallyDrop::take(&mut selection[0].range).ok_or(E_FAIL)?;
        if fetched != 1 || !crate::edit::allowed_input_scope(context, ec, &range) {
            return Err(E_ACCESSDENIED.into());
        }
        let original = read_range(&range, ec)?;
        let foreground = GetForegroundWindow();
        let owner = context
            .GetActiveView()
            .and_then(|v| v.GetWnd())
            .ok()
            .filter(|hwnd| !hwnd.is_invalid() && GetAncestor(*hwnd, GA_ROOT) == foreground)
            .unwrap_or(foreground);
        let (commands, rx) = mpsc::channel();
        let (updates, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&cancelled);
        let id = format!(
            "voice-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let settings = retype_ai::voice::Settings::default();
        std::thread::Builder::new()
            .name("retype-voice-ui".into())
            .spawn(move || {
                let result = retype_ai::client::voice(Command::Start {
                    id: id.clone(),
                    settings,
                });
                let failed = result.is_err();
                let _ = updates.send(result);
                if failed {
                    return;
                }
                loop {
                    if cancel.load(Ordering::Acquire) {
                        let _ = retype_ai::client::voice(Command::Cancel { id });
                        break;
                    }
                    match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            let _ = retype_ai::client::voice(Command::Cancel { id });
                            break;
                        }
                        Ok(true) => {
                            if let Err(e) =
                                retype_ai::client::voice(Command::Stop { id: id.clone() })
                            {
                                let _ = updates.send(Err(e));
                                break;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    let result = retype_ai::client::voice(Command::Poll { id: id.clone() });
                    let done = result.as_ref().map_or(true, |s| {
                        !matches!(s.phase, Phase::Recording | Phase::Recognizing)
                    });
                    if updates.send(result).is_err() || done {
                        break;
                    }
                }
            })
            .map_err(|_| E_FAIL)?;
        let data = Frame {
            state: Arc::downgrade(state),
            context: context.clone(),
            range,
            original,
            epoch: state.epoch.load(Ordering::SeqCst),
            owner,
            commands,
            updates: receiver,
            cancelled,
            snapshot: Snapshot::default(),
            held: held.then(|| state.shortcuts().voice.vk),
            finishing: false,
            applying: false,
            deadline: Instant::now() + Duration::from_secs(180),
            hide_at: None,
            scale: 96,
            scroll: 0,
            max_scroll: 0,
            follow_tail: true,
        };
        let hwnd = create(data, ec)?;
        *lock(&state.voice_window) = Some(hwnd.0 as usize);
        Ok(())
    }
}
fn read_range(range: &ITfRange, ec: u32) -> Result<Vec<u16>> {
    unsafe {
        let cursor = range.Clone()?;
        let mut text = Vec::new();
        loop {
            let mut buf = [0u16; 512];
            let mut count = 0;
            cursor.GetText(ec, TF_TF_MOVESTART, &mut buf, &mut count)?;
            if count == 0 {
                break;
            }
            if count as usize > buf.len() || text.len() + count as usize > 8192 {
                return Err(E_INVALIDARG.into());
            }
            text.extend_from_slice(&buf[..count as usize]);
        }
        Ok(text)
    }
}
unsafe fn frame(hwnd: HWND) -> Option<&'static mut Frame> {
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Frame).as_mut() }
}
fn finish(f: &mut Frame) {
    if !f.finishing {
        f.finishing = true;
        f.held = None;
        f.deadline = Instant::now() + Duration::from_secs(90);
        let _ = f.commands.send(true);
    }
}
fn create(mut data: Frame, ec: u32) -> Result<HWND> {
    unsafe {
        let mut module = HMODULE::default();
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(window_proc as *const () as *const u16),
            &mut module,
        )?;
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: module.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("Retype.Voice.1"),
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("Retype.Voice.1"),
            w!("retype 语音输入"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            Some(data.owner),
            None,
            Some(module.into()),
            None,
        )?;
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        data.scale = dpi;
        let width = 400 * dpi / 96;
        let height = 136 * dpi / 96;
        let mut rect = RECT::default();
        let mut clipped = windows::core::BOOL(0);
        if data
            .context
            .GetActiveView()
            .and_then(|v| v.GetTextExt(ec, &data.range, &mut rect, &mut clipped))
            .is_err()
        {
            let _ = GetWindowRect(data.owner, &mut rect);
        }
        let mut monitor = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(
            MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        );
        let x = rect.left.clamp(
            monitor.rcWork.left,
            (monitor.rcWork.right - width).max(monitor.rcWork.left),
        );
        let y = (rect.bottom + 8).clamp(
            monitor.rcWork.top,
            (monitor.rcWork.bottom - height).max(monitor.rcWork.top),
        );
        let ptr = Box::into_raw(Box::new(data));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as _);
        if SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
        .is_err()
            || SetTimer(Some(hwnd), 91, 100, None) == 0
        {
            let _ = DestroyWindow(hwnd);
            return Err(E_FAIL.into());
        }
        Ok(hwnd)
    }
}
pub(crate) fn replace_verified(
    context: &ITfContext,
    range: &ITfRange,
    ec: u32,
    original: &[u16],
    text: &str,
) -> Result<()> {
    if text.is_empty() || text.len() > 128 * 1024 {
        return Err(E_INVALIDARG.into());
    }
    // SAFETY: ec is a write cookie for this context. Verify selection and privacy before one range write.
    unsafe {
        let mut selected = [TF_SELECTION::default()];
        let mut count = 0;
        context.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selected, &mut count)?;
        let current = ManuallyDrop::take(&mut selected[0].range).ok_or(E_FAIL)?;
        if count != 1
            || current.CompareStart(ec, range, TF_ANCHOR_START)? != 0
            || current.CompareEnd(ec, range, TF_ANCHOR_END)? != 0
            || read_range(range, ec)? != original
            || !crate::edit::allowed_input_scope(context, ec, range)
            || context.GetStatus()?.dwDynamicFlags & TS_SD_READONLY != 0
        {
            return Err(E_ACCESSDENIED.into());
        }
        range.SetText(ec, 0, &text.encode_utf16().collect::<Vec<_>>())?;
        let caret = range.Clone()?;
        caret.Collapse(ec, TF_ANCHOR_END)?;
        let selection = TF_SELECTION {
            range: ManuallyDrop::new(Some(caret)),
            style: TF_SELECTIONSTYLE {
                ase: TF_AE_NONE,
                fInterimChar: false.into(),
            },
        };
        let _ = context.SetSelection(ec, std::slice::from_ref(&selection));
        let _ = ManuallyDrop::into_inner(selection.range);
        Ok(())
    }
}
#[implement(ITfEditSession)]
struct Apply {
    window: usize,
    cancelled: Arc<AtomicBool>,
}
impl ITfEditSession_Impl for Apply_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        guarded(|| {
            if self.cancelled.load(Ordering::Acquire) {
                return Ok(());
            }
            let hwnd = HWND(self.window as *mut _);
            let Some(f) = (unsafe { frame(hwnd) }) else {
                return Ok(());
            };
            let Some(state) = f.state.upgrade() else {
                return Ok(());
            };
            // Copy apartment-owned interfaces before callbacks can destroy this window.
            let context = f.context.clone();
            let range = f.range.clone();
            let original = f.original.clone();
            let text = f.snapshot.text.clone();
            let epoch = f.epoch;
            let owner = f.owner;
            let result = if self.cancelled.load(Ordering::Acquire)
                || !crate::translation::same_target(&state, &context, epoch, owner)
            {
                Err(E_ABORT.into())
            } else {
                replace_verified(&context, &range, ec, &original, &text)
            };
            // Privacy checks may reenter the host; keep them outside a window-data borrow.
            let learnable = result.is_ok() && crate::edit::learnable(&context, ec);
            let Some(f) =
                (unsafe { frame(hwnd) }).filter(|f| Arc::ptr_eq(&f.cancelled, &self.cancelled))
            else {
                return Ok(());
            };
            if result.is_ok() {
                if learnable {
                    if let Some(session) = state.session() {
                        session
                            .backend
                            .record_learning(retype_types::LearningEvent::VoiceCommit {
                                text: text.clone(),
                            });
                    }
                    crate::stats::voice(&text);
                }
                f.hide_at = Some(Instant::now() + Duration::from_millis(800));
            } else {
                f.snapshot.phase = Phase::Error;
                f.snapshot.error = Some("应用拒绝上屏，识别文字保留在此处".into());
            }
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            Ok(())
        })
    }
}
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_ERASEBKGND => LRESULT(1),
            WM_TIMER => {
                let Some(f) = frame(hwnd) else {
                    return LRESULT(0);
                };
                let Some(state) = f.state.upgrade() else {
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                };
                let context = f.context.clone();
                let epoch = f.epoch;
                let owner = f.owner;
                let cancel = Arc::clone(&f.cancelled);
                let expired = f.hide_at.is_some_and(|t| Instant::now() >= t);
                if !crate::translation::same_target(&state, &context, epoch, owner) || expired {
                    dismiss(&state);
                    return LRESULT(0);
                }
                let Some(f) = frame(hwnd).filter(|f| Arc::ptr_eq(&f.cancelled, &cancel)) else {
                    return LRESULT(0);
                };
                if f.held.is_some_and(|vk| GetAsyncKeyState(vk as i32) >= 0) {
                    finish(f);
                }
                while let Ok(result) = f.updates.try_recv() {
                    match result {
                        Ok(s) => f.snapshot = s,
                        Err(e) => {
                            f.snapshot.phase = Phase::Error;
                            f.snapshot.error = Some(e);
                        }
                    }
                }
                if Instant::now() > f.deadline
                    && matches!(f.snapshot.phase, Phase::Recording | Phase::Recognizing)
                {
                    f.snapshot.phase = Phase::Error;
                    f.snapshot.error = Some("语音识别超时，请重试".into());
                    f.cancelled.store(true, Ordering::Release);
                }
                if f.snapshot.phase == Phase::Done && !f.applying {
                    f.applying = true;
                    let session: ITfEditSession = Apply {
                        window: hwnd.0 as usize,
                        cancelled: Arc::clone(&f.cancelled),
                    }
                    .into();
                    let context = f.context.clone();
                    let tid = state.tid.load(Ordering::SeqCst);
                    let result = context.RequestEditSession(
                        tid,
                        &session,
                        TF_ES_ASYNCDONTCARE | TF_ES_READWRITE,
                    );
                    if !result.is_ok_and(|hr| hr.is_ok()) {
                        if let Some(f) = frame(hwnd).filter(|f| Arc::ptr_eq(&f.cancelled, &cancel))
                        {
                            f.snapshot.phase = Phase::Error;
                            f.snapshot.error = Some("应用拒绝上屏，识别文字保留在此处".into());
                        }
                    }
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                if let Some(f) = frame(hwnd) {
                    let delta = ((wp.0 >> 16) as u16) as i16 as i32;
                    f.scroll =
                        (f.scroll - delta * 36 * f.scale / (120 * 96)).clamp(0, f.max_scroll);
                    f.follow_tail = f.scroll == f.max_scroll;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if let Some(f) = frame(hwnd) {
                    let x = (lp.0 & 0xffff) as i32 * 96 / f.scale;
                    let y = ((lp.0 >> 16) & 0xffff) as i32 * 96 / f.scale;
                    if y >= 100 && x > 300 {
                        if let Some(s) = f.state.upgrade() {
                            dismiss(&s);
                        }
                    } else if y >= 100 && f.snapshot.phase == Phase::Recording {
                        finish(f);
                    }
                }
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let target = BeginPaint(hwnd, &mut ps);
                let mut bounds = RECT::default();
                let _ = GetClientRect(hwnd, &mut bounds);
                let dc = CreateCompatibleDC(Some(target));
                let bitmap = CreateCompatibleBitmap(target, bounds.right, bounds.bottom);
                let old_bitmap = SelectObject(dc, bitmap.into());
                if let Some(f) = frame(hwnd) {
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let brush = CreateSolidBrush(COLORREF(0x00fafafa));
                    FillRect(dc, &rect, brush);
                    let _ = DeleteObject(brush.into());
                    SetBkMode(dc, TRANSPARENT);
                    let font = CreateFontW(
                        -12 * f.scale / 96,
                        0,
                        0,
                        0,
                        400,
                        0,
                        0,
                        0,
                        DEFAULT_CHARSET,
                        OUT_DEFAULT_PRECIS,
                        CLIP_DEFAULT_PRECIS,
                        CLEARTYPE_QUALITY,
                        DEFAULT_PITCH.0 as u32,
                        w!("Microsoft YaHei UI"),
                    );
                    let old = SelectObject(dc, font.into());
                    let status = f
                        .snapshot
                        .error
                        .as_deref()
                        .unwrap_or(match f.snapshot.phase {
                            Phase::Recording => "录音中…",
                            Phase::Recognizing => "识别中…",
                            Phase::Done => "已上屏",
                            _ => "已取消",
                        });
                    SetTextColor(dc, COLORREF(0x006f635d));
                    let mut heading = RECT {
                        left: 14 * f.scale / 96,
                        top: 10 * f.scale / 96,
                        right: 380 * f.scale / 96,
                        bottom: 30 * f.scale / 96,
                    };
                    DrawTextW(
                        dc,
                        &mut format!("{status}  {}s", f.snapshot.seconds)
                            .encode_utf16()
                            .collect::<Vec<_>>(),
                        &mut heading,
                        DT_SINGLELINE | DT_NOPREFIX,
                    );
                    if f.snapshot.phase == Phase::Recording {
                        let brush = CreateSolidBrush(COLORREF(0x00968f13));
                        for i in 0..12 {
                            let height = (3.0
                                + f.snapshot.level * 14.0 * (0.4 + (i * 7 % 5) as f32 / 7.0))
                                as i32
                                * f.scale
                                / 96;
                            let bar = RECT {
                                left: (304 + i * 5) * f.scale / 96,
                                top: 20 * f.scale / 96 - height / 2,
                                right: (306 + i * 5) * f.scale / 96,
                                bottom: 20 * f.scale / 96 + height / 2,
                            };
                            FillRect(dc, &bar, brush);
                        }
                        let _ = DeleteObject(brush.into());
                    }
                    let text_font = CreateFontW(
                        -16 * f.scale / 96,
                        0,
                        0,
                        0,
                        400,
                        0,
                        0,
                        0,
                        DEFAULT_CHARSET,
                        OUT_DEFAULT_PRECIS,
                        CLIP_DEFAULT_PRECIS,
                        CLEARTYPE_QUALITY,
                        0,
                        w!("Microsoft YaHei UI"),
                    );
                    SelectObject(dc, text_font.into());
                    let mut textrect = RECT {
                        left: 14 * f.scale / 96,
                        top: 38 * f.scale / 96,
                        right: 386 * f.scale / 96,
                        bottom: 94 * f.scale / 96,
                    };
                    SetTextColor(dc, COLORREF(0x00292320));
                    let mut units = f.snapshot.text.encode_utf16().collect::<Vec<_>>();
                    let mut measured = textrect;
                    DrawTextW(
                        dc,
                        &mut units,
                        &mut measured,
                        DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX,
                    );
                    f.max_scroll =
                        (measured.bottom - measured.top - (textrect.bottom - textrect.top)).max(0);
                    if f.follow_tail {
                        f.scroll = f.max_scroll;
                    } else {
                        f.scroll = f.scroll.clamp(0, f.max_scroll);
                    }
                    let saved = SaveDC(dc);
                    IntersectClipRect(
                        dc,
                        textrect.left,
                        textrect.top,
                        textrect.right,
                        textrect.bottom,
                    );
                    textrect.top -= f.scroll;
                    textrect.bottom = measured.bottom - f.scroll;
                    DrawTextW(dc, &mut units, &mut textrect, DT_WORDBREAK | DT_NOPREFIX);
                    let _ = RestoreDC(dc, saved);
                    SelectObject(dc, font.into());
                    let mut buttons = RECT {
                        left: 14 * f.scale / 96,
                        top: 108 * f.scale / 96,
                        right: 386 * f.scale / 96,
                        bottom: 130 * f.scale / 96,
                    };
                    DrawTextW(
                        dc,
                        &mut (if f.snapshot.phase == Phase::Recording {
                            "结束录音                                   取消"
                        } else {
                            "                                                    返回"
                        })
                        .encode_utf16()
                        .collect::<Vec<_>>(),
                        &mut buttons,
                        DT_SINGLELINE,
                    );
                    SelectObject(dc, old);
                    let _ = DeleteObject(font.into());
                    let _ = DeleteObject(text_font.into());
                }
                let _ = BitBlt(
                    target,
                    0,
                    0,
                    bounds.right,
                    bounds.bottom,
                    Some(dc),
                    0,
                    0,
                    SRCCOPY,
                );
                SelectObject(dc, old_bitmap);
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(dc);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Frame;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if !ptr.is_null() {
                    let data = Box::from_raw(ptr);
                    if let Some(state) = data.state.upgrade() {
                        let mut slot = lock(&state.voice_window);
                        if *slot == Some(hwnd.0 as usize) {
                            *slot = None;
                        }
                    }
                    drop(data);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }))
    .unwrap_or(LRESULT(0))
}
