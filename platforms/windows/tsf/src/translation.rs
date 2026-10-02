//! Full-context translation. COM objects stay on the host's TSF apartment.
use crate::tip::{guarded, lock, TipState};
use retype_ai::protocol::{Operation, Response, MAX_TEXT};
use std::{
    mem::ManuallyDrop,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Weak,
    },
    time::{Duration, Instant},
};
use windows::{
    core::{implement, w, Interface, Result, PCWSTR},
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{DataExchange::*, LibraryLoader::*, Memory::*, Ole::CF_UNICODETEXT},
        UI::{HiDpi::GetDpiForWindow, TextServices::*, WindowsAndMessaging::*},
    },
};
const TIMER: usize = 88;
const STATUS_WIDTH: i32 = 360;
const STATUS_HEIGHT: i32 = 30;
const PREVIEW_HEIGHT: i32 = 208;
const ERROR_NOTICE_DURATION: Duration = Duration::from_secs(3);
struct Frame {
    state: Weak<TipState>,
    context: ITfContext,
    epoch: u32,
    original: Vec<u16>,
    response: Option<mpsc::Receiver<std::result::Result<Response, String>>>,
    cancelled: Arc<AtomicBool>,
    translation: Option<String>,
    message: String,
    scroll: i32,
    max_scroll: i32,
    owner: HWND,
    applying: bool,
    applied: bool,
    busy: bool,
    preview: bool,
    spinner_phase: u32,
    hide_at: Option<Instant>,
}
impl Drop for Frame {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}
pub(crate) fn dismiss(state: &TipState) {
    let handle = lock(&state.translation_window).take();
    if let Some(handle) = handle {
        // SAFETY: this window is created and destroyed on this TSF apartment.
        unsafe {
            let _ = DestroyWindow(HWND(handle as *mut _));
        }
    }
}
pub(crate) fn capture(state: &Arc<TipState>, context: &ITfContext, ec: u32) -> Result<()> {
    dismiss(state);
    let snapshot = if state.session().is_some_and(|session| session.shadow) {
        Err("此输入框暂不支持全文翻译".into())
    } else {
        read(context, ec)
    };
    let (original, message) = match snapshot {
        Ok(text) => (text, "翻译中…".to_owned()),
        Err(e) => (Vec::new(), e),
    };
    let owner = unsafe { context.GetActiveView().and_then(|v| v.GetWnd()) }.unwrap_or_default();
    let cancelled = Arc::new(AtomicBool::new(false));
    let data = Frame {
        state: Arc::downgrade(state),
        context: context.clone(),
        epoch: state.epoch.load(Ordering::SeqCst),
        original: original.clone(),
        response: None,
        cancelled: Arc::clone(&cancelled),
        translation: None,
        message,
        scroll: 0,
        max_scroll: 0,
        owner,
        applying: false,
        applied: false,
        busy: !original.is_empty(),
        preview: false,
        spinner_phase: 0,
        hide_at: None,
    };
    let hwnd = create(data, context, ec)?;
    *lock(&state.translation_window) = Some(hwnd.0 as usize);
    if !original.is_empty() {
        let text = String::from_utf16(&original).map_err(|_| E_FAIL)?;
        let (tx, rx) = mpsc::channel();
        // SAFETY: the frame is owned by hwnd on this thread; worker receives only text/cancel flag.
        unsafe {
            if let Some(frame) = frame(hwnd) {
                frame.response = Some(rx);
            }
        }
        if std::thread::Builder::new()
            .name("retype-translate".into())
            .spawn(move || {
                let result = retype_ai::client::run(Operation::Translate { text }, &cancelled);
                let _ = tx.send(result);
            })
            .is_err()
        {
            status(hwnd, "无法创建翻译任务");
        }
    }
    Ok(())
}
/// Read to both context anchors in bounded chunks; never use IGNOREEND or a truncated prefix.
pub(crate) fn read(context: &ITfContext, ec: u32) -> std::result::Result<Vec<u16>, String> {
    let status = unsafe { context.GetStatus() };
    crate::settings_log::event(
        "launcher",
        "translation_context",
        0,
        format!(
            "static_flags={} dynamic_flags={} hresult={}",
            status.as_ref().map_or(0, |s| s.dwStaticFlags),
            status.as_ref().map_or(0, |s| s.dwDynamicFlags),
            status.as_ref().err().map_or(0, |e| e.code().0)
        ),
    );
    // SAFETY: ec is a granted read/write or read cookie for this context.
    unsafe {
        if context
            .GetStatus()
            .map_err(|_| "无法读取输入框状态")?
            .dwDynamicFlags
            & TS_SD_READONLY
            != 0
        {
            return Err("输入框为只读".into());
        }
        let range = context
            .GetStart(ec)
            .map_err(|_| "此应用不支持输入框全文读取")?;
        let end = context
            .GetEnd(ec)
            .map_err(|_| "此应用不支持输入框全文读取")?;
        range
            .ShiftEndToRange(ec, &end, TF_ANCHOR_END)
            .map_err(|_| "无法确定输入框全文范围")?;
        if !crate::edit::allowed_input_scope(context, ec, &range) {
            return Err("此输入框的密码或隐私属性不允许读取全文".into());
        }
        // ACP bounds confirm this is a complete text-store range rather than a composition fragment.
        let start_acp: ITfRangeACP = range.cast().map_err(|_| "此输入框无法确认全文范围")?;
        let mut start = 0;
        let mut length = 0;
        start_acp
            .GetExtent(&mut start, &mut length)
            .map_err(|_| "此输入框无法确认全文范围")?;
        if start != 0 || length < 0 || length as usize > MAX_TEXT {
            return Err("输入框范围不完整或文字过长".into());
        }
        let cursor = range.Clone().map_err(|_| "无法读取全文")?;
        let mut text = Vec::new();
        loop {
            let mut buffer = [0u16; 2048];
            let mut count = 0;
            cursor
                .GetText(ec, TF_TF_MOVESTART, &mut buffer, &mut count)
                .map_err(|_| "无法读取输入框全文")?;
            if count as usize > buffer.len() {
                return Err("应用返回的文字长度无效".into());
            }
            if count == 0 {
                break;
            }
            text.extend_from_slice(&buffer[..count as usize]);
            if text.len() > MAX_TEXT {
                return Err("输入框文字过长，原文保持不变".into());
            }
        }
        if text.len() != length as usize {
            return Err("应用未提供完整文字，原文保持不变".into());
        }
        validate_text(&text)?;
        Ok(text)
    }
}
fn validate_text(text: &[u16]) -> std::result::Result<(), String> {
    let text = String::from_utf16(text).map_err(|_| "输入框文字编码无效")?;
    if text.trim().is_empty() {
        return Err("输入框为空".into());
    }
    if text.contains(['\u{fffc}', '\0']) {
        return Err("输入框包含嵌入对象，暂不支持全文替换".into());
    }
    Ok(())
}
#[cfg(test)]
pub(crate) fn replace_verified(
    context: &ITfContext,
    ec: u32,
    original: &[u16],
    text: &str,
) -> std::result::Result<(), String> {
    replace_checked(context, ec, original, text, || true)
}
fn replace_checked(
    context: &ITfContext,
    ec: u32,
    original: &[u16],
    text: &str,
    current: impl Fn() -> bool,
) -> std::result::Result<(), String> {
    if read(context, ec)? != original {
        return Err("原文已变化，未替换".into());
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() > MAX_TEXT {
        return Err("译文过长，未替换".into());
    }
    validate_text(&units)?;
    // SAFETY: caller holds the verified context's write cookie; one range write, no pre-clear.
    unsafe {
        let start = context.GetStart(ec).map_err(|_| "无法读取全文范围")?;
        let end = context.GetEnd(ec).map_err(|_| "无法读取全文范围")?;
        start
            .ShiftEndToRange(ec, &end, TF_ANCHOR_END)
            .map_err(|_| "无法读取全文范围")?;
        if !current() {
            return Err("翻译已取消或输入框已切换，未替换".into());
        }
        start.SetText(ec, 0, &units).map_err(|error| {
            crate::settings_log::event(
                "launcher",
                "translation_replace_failed",
                0,
                format!("stage=set_text hresult={:08x}", error.code().0),
            );
            "应用拒绝替换，原文保持不变"
        })?;
        if start.Collapse(ec, TF_ANCHOR_END).is_err() {
            return Ok(());
        }
        let selection = TF_SELECTION {
            range: ManuallyDrop::new(Some(start)),
            style: TF_SELECTIONSTYLE {
                ase: TF_AE_NONE,
                fInterimChar: false.into(),
            },
        };
        let result = context.SetSelection(ec, std::slice::from_ref(&selection));
        let mut selection = selection;
        drop(ManuallyDrop::take(&mut selection.range));
        // The replacement succeeded even if a host refuses to move its caret.
        let _ = result;
    }
    Ok(())
}
fn same_target(state: &TipState, context: &ITfContext, epoch: u32, owner: HWND) -> bool {
    if !state.is_activated()
        || state.epoch.load(Ordering::SeqCst) != epoch
        || lock(&state.composition).is_some()
    {
        return false;
    }
    let Some(manager) = lock(&state.thread_mgr).clone() else {
        return false;
    };
    // SAFETY: read-only focus checks on the owning COM apartment.
    unsafe {
        let focused = manager
            .GetFocus()
            .and_then(|d| d.GetTop())
            .is_ok_and(|c| c == *context);
        let foreground = GetForegroundWindow();
        let root = GetAncestor(owner, GA_ROOT);
        focused && !owner.is_invalid() && !root.is_invalid() && foreground == root
    }
}
fn request_apply(hwnd: HWND) {
    let data = unsafe { frame(hwnd) }.and_then(|f| {
        if f.applying || f.applied {
            return None;
        }
        let state = f.state.upgrade()?;
        let text = f.translation.clone()?;
        f.applying = true;
        Some((
            state,
            f.context.clone(),
            f.original.clone(),
            text,
            f.epoch,
            f.owner,
            Arc::clone(&f.cancelled),
        ))
    });
    let Some((state, context, original, text, epoch, owner, cancelled)) = data else {
        return;
    };
    if !same_target(&state, &context, epoch, owner) {
        status(hwnd, "输入框已切换或正在输入，原文保持不变");
        return;
    }
    let session: ITfEditSession = Apply {
        state: Arc::downgrade(&state),
        context: context.clone(),
        original,
        text,
        epoch,
        owner,
        cancelled,
        window: hwnd.0 as usize,
    }
    .into();
    // SAFETY: TSF schedules this object on the current context apartment; no worker holds COM.
    let result = unsafe {
        context.RequestEditSession(
            state.tid.load(Ordering::SeqCst),
            &session,
            TF_ES_ASYNCDONTCARE | TF_ES_READWRITE,
        )
    };
    let failure = match result {
        Ok(session) if session.is_ok() => None,
        Ok(session) => Some(("session_result", session.0)),
        Err(error) => Some(("request_edit_session", error.code().0)),
    };
    if let Some((stage, code)) = failure {
        crate::settings_log::event(
            "launcher",
            "translation_replace_failed",
            0,
            format!("stage={stage} hresult={code:08x}"),
        );
        copy_fallback(hwnd);
    }
}
#[implement(ITfEditSession)]
struct Apply {
    state: Weak<TipState>,
    context: ITfContext,
    original: Vec<u16>,
    text: String,
    epoch: u32,
    owner: HWND,
    cancelled: Arc<AtomicBool>,
    window: usize,
}
impl ITfEditSession_Impl for Apply_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        guarded(|| {
            let hwnd = HWND(self.window as *mut _);
            if self.cancelled.load(Ordering::Relaxed) {
                return Ok(());
            }
            let Some(state) = self.state.upgrade() else {
                return Ok(());
            };
            if !same_target(&state, &self.context, self.epoch, self.owner) {
                status(hwnd, "输入框已切换，原文保持不变");
                return Ok(());
            }
            let result = replace_checked(&self.context, ec, &self.original, &self.text, || {
                !self.cancelled.load(Ordering::Relaxed)
                    && same_target(&state, &self.context, self.epoch, self.owner)
            });
            if result.is_ok() {
                status(hwnd, "翻译完成");
            } else if result.is_err() {
                copy_fallback(hwnd);
            }
            // Translation is not typing activity and does not enter personal word learning.
            Ok(())
        })
    }
}
fn copy_fallback(hwnd: HWND) {
    copy_fallback_with(hwnd, copy_translation);
}
fn copy_fallback_with(hwnd: HWND, copy: impl FnOnce(HWND, &str) -> Result<()>) {
    // Copy only an already validated, completed translation from a live request.
    let text = unsafe { frame(hwnd) }.and_then(|f| {
        if f.cancelled.load(Ordering::Relaxed) || f.applied {
            return None;
        }
        f.preview = false;
        f.translation.clone()
    });
    let Some(text) = text else { return };
    let result = copy(hwnd, &text);
    crate::settings_log::event(
        "launcher",
        "translation_clipboard_fallback",
        0,
        format!(
            "success={} hresult={:08x}",
            result.is_ok(),
            result.as_ref().err().map_or(0, |e| e.code().0)
        ),
    );
    status(
        hwnd,
        if result.is_ok() {
            "译文已复制"
        } else {
            "替换失败，无法复制译文"
        },
    );
}

fn copy_translation(hwnd: HWND, text: &str) -> Result<()> {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    validate_text(&units).map_err(|_| E_INVALIDARG)?;
    if units.len() > MAX_TEXT {
        return Err(E_INVALIDARG.into());
    }
    units.push(0);
    // SAFETY: Allocate movable clipboard storage, copy its bounded UTF-16 data,
    // and transfer ownership only after SetClipboardData succeeds. Always close
    // the clipboard and free memory on failures. No clipboard contents are read.
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * std::mem::size_of::<u16>())?;
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            let error = windows::core::Error::from_thread();
            let _ = GlobalFree(Some(memory));
            return Err(error);
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), pointer.cast::<u16>(), units.len());
        let _ = GlobalUnlock(memory);
        if let Err(error) = OpenClipboard(Some(hwnd)) {
            let _ = GlobalFree(Some(memory));
            return Err(error);
        }
        let result = EmptyClipboard()
            .and_then(|_| SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(memory.0))));
        let _ = CloseClipboard();
        if result.is_err() {
            let _ = GlobalFree(Some(memory));
        }
        result.map(|_| ())
    }
}
unsafe fn frame<'a>(hwnd: HWND) -> Option<&'a mut Frame> {
    // SAFETY: GWLP_USERDATA is our exclusive Box<Frame>, accessed only on its window thread.
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Frame;
    unsafe { pointer.as_mut() }
}
fn status(hwnd: HWND, message: &str) {
    // SAFETY: called only on the window owner thread, outside any existing frame borrow.
    let preview = unsafe {
        if let Some(f) = frame(hwnd) {
            f.message = message.into();
            f.applying = false;
            f.busy = false;
            if message == "翻译完成" || message == "译文已复制" {
                f.applied = true;
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            let preview = f.preview && f.translation.is_some() && !f.applied;
            f.hide_at = if preview {
                None
            } else {
                Some(
                    Instant::now()
                        + if message == "翻译完成" {
                            Duration::from_millis(1400)
                        } else {
                            ERROR_NOTICE_DURATION
                        },
                )
            };
            preview
        } else {
            false
        }
    };
    resize_notice(hwnd, preview);
}
fn resize_notice(hwnd: HWND, preview: bool) {
    // SAFETY: resize the non-activating owned window within its monitor work area.
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return;
        }
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        let content = frame(hwnd).map(|f| (f.message.clone(), f.busy || f.applying));
        let width = if preview {
            STATUS_WIDTH * dpi / 96
        } else if let Some((message, loading)) = content {
            notice_width(hwnd, dpi, &message, loading)
        } else {
            STATUS_WIDTH * dpi / 96
        };
        let height = (if preview {
            PREVIEW_HEIGHT
        } else {
            STATUS_HEIGHT
        }) * dpi
            / 96;
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
            return;
        }
        let top = rect.top.clamp(
            info.rcWork.top,
            (info.rcWork.bottom - height).max(info.rcWork.top),
        );
        let left = rect.left.clamp(
            info.rcWork.left,
            (info.rcWork.right - width).max(info.rcWork.left),
        );
        let _ = SetWindowPos(
            hwnd,
            None,
            left,
            top,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        round_notice(hwnd, width, height, dpi);
    }
}
fn round_notice(hwnd: HWND, width: i32, height: i32, dpi: i32) {
    // SAFETY: The system owns a successfully assigned region; failed assignment
    // retains ownership here. This clips the popup's corners without a shadow.
    unsafe {
        let radius = 12 * dpi / 96;
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius, radius);
        if SetWindowRgn(hwnd, Some(region), true) == 0 {
            let _ = DeleteObject(region.into());
        }
    }
}
fn notice_font(dpi: i32) -> HFONT {
    // SAFETY: Fixed font face; callers own and delete the returned GDI font.
    unsafe {
        CreateFontW(
            -12 * dpi / 96,
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
            w!("Microsoft YaHei"),
        )
    }
}
fn notice_width(hwnd: HWND, dpi: i32, message: &str, loading: bool) -> i32 {
    // SAFETY: Temporary owner-thread DC and font, with no frame borrow held.
    unsafe {
        let dc = GetDC(Some(hwnd));
        if dc.is_invalid() {
            return STATUS_WIDTH * dpi / 96;
        }
        let font = notice_font(dpi);
        let old = SelectObject(dc, font.into());
        let text: Vec<u16> = message.encode_utf16().collect();
        let mut size = SIZE::default();
        let measured = text.is_empty() || GetTextExtentPoint32W(dc, &text, &mut size).as_bool();
        SelectObject(dc, old);
        let _ = DeleteObject(font.into());
        ReleaseDC(Some(hwnd), dc);
        if !measured {
            return STATUS_WIDTH * dpi / 96;
        }
        (size.cx + (if loading { 30 } else { 10 } + 10) * dpi / 96)
            .clamp(100 * dpi / 96, STATUS_WIDTH * dpi / 96)
    }
}
fn create(mut data: Frame, context: &ITfContext, ec: u32) -> Result<HWND> {
    // SAFETY: this module owns window class, user data and all GDI resources.
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
            lpszClassName: w!("Retype.Translation.1"),
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("Retype.Translation.1"),
            w!("retype 翻译"),
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
        let width = notice_width(hwnd, dpi, &data.message, data.busy);
        let height = STATUS_HEIGHT * dpi / 96;
        let mut rect = RECT::default();
        let mut selected = [TF_SELECTION::default()];
        let mut count = 0;
        let _ = context.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selected, &mut count);
        let range = ManuallyDrop::take(&mut selected[0].range);
        let mut clipped = windows_core::BOOL(0);
        let anchored = range.as_ref().is_some_and(|r| {
            context
                .GetActiveView()
                .and_then(|v| v.GetTextExt(ec, r, &mut rect, &mut clipped))
                .is_ok()
        });
        if !anchored {
            let _ = GetWindowRect(data.owner, &mut rect);
        }
        let monitor = MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let x = rect.left.clamp(
            info.rcWork.left,
            (info.rcWork.right - width).max(info.rcWork.left),
        );
        let y = (rect.bottom + 8).clamp(
            info.rcWork.top,
            (info.rcWork.bottom - height).max(info.rcWork.top),
        );
        data.hide_at = (!data.busy).then(|| Instant::now() + ERROR_NOTICE_DURATION);
        let pointer = Box::into_raw(Box::new(data));
        #[cfg(target_pointer_width = "64")]
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as isize);
        #[cfg(target_pointer_width = "32")]
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as i32);
        round_notice(hwnd, width, height, dpi);
        if let Err(error) = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        ) {
            let _ = DestroyWindow(hwnd);
            return Err(error);
        }
        if SetTimer(Some(hwnd), TIMER, 100, None) == 0 {
            let error = windows::core::Error::from_thread();
            crate::settings_log::event(
                "launcher",
                "translation_timer_failed",
                0,
                format!("hresult={:08x}", error.code().0),
            );
            let _ = DestroyWindow(hwnd);
            return Err(error);
        }
        Ok(hwnd)
    }
}
unsafe fn paint(hwnd: HWND) {
    // SAFETY: Paint/DC/bitmap lifetimes are paired, and selected objects are
    // restored before deletion. The window only receives a completed frame.
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let target = BeginPaint(hwnd, &mut ps);
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let buffer = CreateCompatibleDC(Some(target));
        let bitmap = CreateCompatibleBitmap(target, rect.right.max(1), rect.bottom.max(1));
        let buffered = !buffer.is_invalid() && !bitmap.is_invalid();
        let previous_bitmap = if buffered {
            Some(SelectObject(buffer, bitmap.into()))
        } else {
            None
        };
        let dc = if buffered { buffer } else { target };
        let brush = CreateSolidBrush(COLORREF(0x00fafafa));
        FillRect(dc, &rect, brush);
        let _ = DeleteObject(brush.into());
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        let pad = 12 * dpi / 96;
        let bar = STATUS_HEIGHT * dpi / 96;
        let font = notice_font(dpi);
        let old = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00807870));
        if let Some(f) = frame(hwnd) {
            let loading = f.busy || f.applying;
            if loading {
                draw_spinner(
                    dc,
                    15 * dpi / 96,
                    bar / 2,
                    5 * dpi / 96,
                    f.spinner_phase,
                    dpi,
                );
            }
            if f.preview && !f.applied {
                let close_pen = CreatePen(PS_SOLID, (dpi / 96).max(1), COLORREF(0x008a7c6c));
                let old_pen = SelectObject(dc, close_pen.into());
                let center = rect.right - 20 * dpi / 96;
                let half = 4 * dpi / 96;
                let _ = MoveToEx(dc, center - half, bar / 2 - half, None);
                let _ = LineTo(dc, center + half, bar / 2 + half);
                let _ = MoveToEx(dc, center + half, bar / 2 - half, None);
                let _ = LineTo(dc, center - half, bar / 2 + half);
                SelectObject(dc, old_pen);
                let _ = DeleteObject(close_pen.into());
            }
            let mut heading = RECT {
                left: if loading {
                    30 * dpi / 96
                } else {
                    10 * dpi / 96
                },
                top: 0,
                right: rect.right - (if f.preview && !f.applied { 38 } else { 10 }) * dpi / 96,
                bottom: bar,
            };
            let mut message: Vec<u16> = f.message.encode_utf16().collect();
            if !message.is_empty() {
                DrawTextW(
                    dc,
                    &mut message,
                    &mut heading,
                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
            }
            let top = bar;
            let bottom = rect.bottom - 40 * dpi / 96;
            let saved = SaveDC(dc);
            IntersectClipRect(dc, pad, top, rect.right - pad, bottom);
            let mut body = RECT {
                left: pad,
                top: top - f.scroll,
                right: rect.right - pad,
                bottom: i32::MAX / 4,
            };
            let mut text: Vec<u16> = f
                .translation
                .as_deref()
                .unwrap_or("")
                .encode_utf16()
                .collect();
            let mut measured = RECT {
                left: pad,
                top: 0,
                right: rect.right - pad,
                bottom: 0,
            };
            // DrawTextW still dereferences its string pointer for a zero-length
            // slice. An empty Vec supplies dangling address 0x2, which crashes
            // the host while the progress/error window has no translation yet.
            f.max_scroll = 0;
            if f.preview && !f.applied && !text.is_empty() && bottom > top {
                DrawTextW(
                    dc,
                    &mut text,
                    &mut measured,
                    DT_WORDBREAK | DT_NOPREFIX | DT_CALCRECT,
                );
                f.max_scroll = (measured.bottom - (bottom - top)).max(0);
                DrawTextW(dc, &mut text, &mut body, DT_WORDBREAK | DT_NOPREFIX);
            }
            let _ = RestoreDC(dc, saved);
            if f.preview && !f.applied && !f.applying {
                let mut button = RECT {
                    left: rect.right - 84 * dpi / 96,
                    top: bottom,
                    right: rect.right - pad,
                    bottom: rect.bottom - 8 * dpi / 96,
                };
                DrawTextW(
                    dc,
                    &mut "替换".encode_utf16().collect::<Vec<_>>(),
                    &mut button,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
            }
        }
        SelectObject(dc, old);
        let _ = DeleteObject(font.into());
        if let Some(previous) = previous_bitmap {
            // BeginPaint clips this copy to the invalid region, so animation
            // frames never touch the text or the rest of the notice.
            let _ = BitBlt(
                target,
                0,
                0,
                rect.right,
                rect.bottom,
                Some(buffer),
                0,
                0,
                SRCCOPY,
            );
            SelectObject(buffer, previous);
        }
        if !bitmap.is_invalid() {
            let _ = DeleteObject(bitmap.into());
        }
        if !buffer.is_invalid() {
            let _ = DeleteDC(buffer);
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

fn draw_spinner(dc: HDC, x: i32, y: i32, radius: i32, phase: u32, dpi: i32) {
    // SAFETY: Owner-thread paint DC; restore selected objects before deleting pens.
    unsafe {
        let brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        let track = CreatePen(PS_SOLID, (2 * dpi / 96).max(1), COLORREF(0x00e9e5df));
        let pen = SelectObject(dc, track.into());
        let _ = Ellipse(dc, x - radius, y - radius, x + radius, y + radius);
        let active = CreatePen(PS_SOLID, (dpi / 96).max(1), COLORREF(0x00a09080));
        SelectObject(dc, active.into());
        let angle = phase as f32 * std::f32::consts::TAU / 12.0;
        let end = angle + std::f32::consts::TAU * 0.7;
        let _ = Arc(
            dc,
            x - radius,
            y - radius,
            x + radius,
            y + radius,
            x + (radius as f32 * angle.cos()).round() as i32,
            y + (radius as f32 * angle.sin()).round() as i32,
            x + (radius as f32 * end.cos()).round() as i32,
            y + (radius as f32 * end.sin()).round() as i32,
        );
        SelectObject(dc, pen);
        SelectObject(dc, brush);
        let _ = DeleteObject(track.into());
        let _ = DeleteObject(active.into());
    }
}

fn spinner_dirty_rect(dpi: i32) -> RECT {
    RECT {
        left: 7 * dpi / 96,
        top: (STATUS_HEIGHT / 2 - 8) * dpi / 96,
        right: 23 * dpi / 96,
        bottom: (STATUS_HEIGHT / 2 + 8) * dpi / 96,
    }
}

#[cfg(test)]
pub(crate) fn paint_status_window_states(context: &ITfContext) -> Result<()> {
    // SAFETY: Isolated message-only window on the test COM apartment. It never
    // appears on the desktop or sends input; it exercises the real GDI painter.
    unsafe {
        let module = GetModuleHandleW(None)?;
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: module.into(),
            lpszClassName: w!("Retype.Translation.Paint.Test"),
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("Retype.Translation.Paint.Test"),
            w!("paint test"),
            WS_OVERLAPPED,
            0,
            0,
            480,
            120,
            Some(HWND_MESSAGE),
            None,
            Some(module.into()),
            None,
        )?;
        let data = Frame {
            state: Weak::new(),
            context: context.clone(),
            epoch: 0,
            original: Vec::new(),
            response: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            translation: None,
            message: String::new(),
            scroll: 0,
            max_scroll: 0,
            owner: HWND::default(),
            applying: false,
            applied: false,
            busy: false,
            preview: false,
            spinner_phase: 0,
            hide_at: None,
        };
        let pointer = Box::into_raw(Box::new(data));
        #[cfg(target_pointer_width = "64")]
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as isize);
        #[cfg(target_pointer_width = "32")]
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as i32);
        let result = (|| -> Result<()> {
            let short = notice_width(hwnd, 96, "翻译中…", true);
            let long = notice_width(hwnd, 96, "输入框已切换或正在输入，原文保持不变", false);
            assert!(
                short < long,
                "Short notices should fit their text instead of occupying the full width"
            );
            for (message, text, height) in [
                ("翻译中…", None, STATUS_HEIGHT),
                ("读取失败", None, STATUS_HEIGHT),
                ("", Some(""), PREVIEW_HEIGHT),
                ("译文预览", Some("你好，这是一个翻译测试。"), PREVIEW_HEIGHT),
            ] {
                SetWindowPos(hwnd, None, 0, 0, 480, height, SWP_NOACTIVATE | SWP_NOZORDER)?;
                {
                    let data = frame(hwnd).ok_or(E_FAIL)?;
                    data.message = message.into();
                    data.translation = text.map(str::to_owned);
                    data.busy = message == "翻译中…";
                    data.preview = height == PREVIEW_HEIGHT;
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                paint(hwnd);
                if message == "翻译中…" {
                    // A lightweight notice has no close hit target at its right edge.
                    window_proc(hwnd, WM_LBUTTONUP, WPARAM(0), LPARAM((10 << 16) | 479));
                    assert!(frame(hwnd).is_some());
                    for phase in 1..=12 {
                        window_proc(hwnd, WM_TIMER, WPARAM(TIMER), LPARAM(0));
                        assert_eq!(frame(hwnd).ok_or(E_FAIL)?.spinner_phase, phase % 12);
                        // Message-only test windows have no visible update region.
                        // Verify the production dirty rectangle stays before the
                        // caption at common display scales while painting each phase.
                        for dpi in [96, 120, 144, 192] {
                            let updated = spinner_dirty_rect(dpi);
                            assert!(updated.right < 30 * dpi / 96);
                            assert!(updated.top >= 0);
                            assert!(updated.bottom <= STATUS_HEIGHT * dpi / 96);
                        }
                        paint(hwnd);
                    }
                }
            }
            // Exercise fallback state transitions without changing the user's
            // real clipboard. Only the clipboard API boundary is substituted.
            {
                let data = frame(hwnd).ok_or(E_FAIL)?;
                data.translation = Some("完整译文\n第二段😀".into());
                data.applied = false;
                data.preview = true;
            }
            copy_fallback_with(hwnd, |_, text| {
                assert_eq!(text, "完整译文\n第二段😀");
                Err(E_ACCESSDENIED.into())
            });
            assert_eq!(frame(hwnd).ok_or(E_FAIL)?.message, "替换失败，无法复制译文");
            assert!(!frame(hwnd).ok_or(E_FAIL)?.applied);
            copy_fallback_with(hwnd, |_, text| {
                assert_eq!(text, "完整译文\n第二段😀");
                Ok(())
            });
            assert_eq!(frame(hwnd).ok_or(E_FAIL)?.message, "译文已复制");
            assert!(frame(hwnd).ok_or(E_FAIL)?.applied);
            assert!(!frame(hwnd).ok_or(E_FAIL)?.preview);
            let copied_again = std::cell::Cell::new(false);
            copy_fallback_with(hwnd, |_, _| {
                copied_again.set(true);
                Ok(())
            });
            assert!(!copied_again.get());
            frame(hwnd).ok_or(E_FAIL)?.applied = false;
            frame(hwnd)
                .ok_or(E_FAIL)?
                .cancelled
                .store(true, Ordering::Relaxed);
            copy_fallback_with(hwnd, |_, _| {
                copied_again.set(true);
                Ok(())
            });
            assert!(!copied_again.get());
            // A stopped/error notice expires even when no worker response will
            // ever arrive (for example an empty field rejected during capture).
            {
                let data = frame(hwnd).ok_or(E_FAIL)?;
                data.message = "输入框为空".into();
                data.busy = false;
                data.applying = false;
                data.hide_at = Some(Instant::now() + ERROR_NOTICE_DURATION);
            }
            window_proc(hwnd, WM_TIMER, WPARAM(TIMER), LPARAM(0));
            assert!(
                IsWindow(Some(hwnd)).as_bool(),
                "Do not close before the deadline"
            );
            frame(hwnd).ok_or(E_FAIL)?.hide_at = Some(Instant::now() - Duration::from_millis(1));
            window_proc(hwnd, WM_TIMER, WPARAM(TIMER), LPARAM(0));
            assert!(
                !IsWindow(Some(hwnd)).as_bool(),
                "Expired errors must destroy the notice"
            );
            Ok(())
        })();
        if IsWindow(Some(hwnd)).as_bool() {
            let _ = DestroyWindow(hwnd);
        }
        result
    }
}
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: window messages execute on the owning TSF apartment.
        unsafe {
            match msg {
                WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
                WM_PAINT => {
                    paint(hwnd);
                    LRESULT(0)
                }
                WM_ERASEBKGND => LRESULT(1),
                WM_TIMER if wp.0 == TIMER => {
                    if frame(hwnd).is_some_and(|f| {
                        f.hide_at.is_some_and(|deadline| Instant::now() >= deadline)
                    }) {
                        let _ = DestroyWindow(hwnd);
                        return LRESULT(0);
                    }
                    let loading = frame(hwnd).is_some_and(|f| {
                        if f.busy || f.applying {
                            f.spinner_phase = (f.spinner_phase + 1) % 12;
                            true
                        } else {
                            false
                        }
                    });
                    if loading {
                        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
                        let spinner = spinner_dirty_rect(dpi);
                        let _ = InvalidateRect(Some(hwnd), Some(&spinner), false);
                    }
                    let response = frame(hwnd).and_then(|f| f.response.as_ref()?.try_recv().ok());
                    if let Some(response) = response {
                        let apply = if let Some(f) = frame(hwnd) {
                            f.response = None;
                            f.busy = false;
                            match response {
                                Ok(Response::Translation { text, preview }) => {
                                    f.translation = Some(text);
                                    f.preview = preview;
                                    f.message = if preview {
                                        "译文预览"
                                    } else {
                                        "翻译完成，正在替换…"
                                    }
                                    .into();
                                    !preview
                                }
                                Err(e) => {
                                    f.message = e;
                                    false
                                }
                                _ => {
                                    f.message = "翻译服务返回无效结果".into();
                                    false
                                }
                            }
                        } else {
                            false
                        };
                        if !apply {
                            let preview =
                                frame(hwnd).is_some_and(|f| f.preview && f.translation.is_some());
                            resize_notice(hwnd, preview);
                            if let Some(f) = frame(hwnd) {
                                f.hide_at =
                                    (!preview).then(|| Instant::now() + ERROR_NOTICE_DURATION);
                            }
                        }
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        if apply {
                            request_apply(hwnd);
                        }
                    }
                    LRESULT(0)
                }
                WM_MOUSEWHEEL => {
                    if let Some(f) = frame(hwnd) {
                        let delta = (wp.0 >> 16) as u16 as i16 as i32;
                        f.scroll = (f.scroll - delta / 120 * 48).clamp(0, f.max_scroll);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    LRESULT(0)
                }
                WM_LBUTTONUP => {
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let x = (lp.0 & 0xffff) as u16 as i16 as i32;
                    let y = ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32;
                    let dpi = GetDpiForWindow(hwnd).max(96) as i32;
                    if x >= rect.right - 38 * dpi / 96
                        && y < STATUS_HEIGHT * dpi / 96
                        && frame(hwnd).is_some_and(|f| f.preview && !f.applied)
                    {
                        let _ = DestroyWindow(hwnd);
                    } else if x >= rect.right - 84 * dpi / 96
                        && y >= rect.bottom - 40 * dpi / 96
                        && frame(hwnd).is_some_and(|f| f.preview && !f.applied && !f.applying)
                    {
                        request_apply(hwnd);
                    }
                    LRESULT(0)
                }
                WM_NCDESTROY => {
                    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Frame;
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    if !pointer.is_null() {
                        let data = Box::from_raw(pointer);
                        if let Some(state) = data.state.upgrade() {
                            let mut window = lock(&state.translation_window);
                            if *window == Some(hwnd.0 as usize) {
                                *window = None;
                            }
                        }
                        drop(data);
                    }
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }));
    result.unwrap_or(LRESULT(0))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_and_invalid_text_is_rejected() {
        assert!(validate_text(&[0xfffc]).is_err());
        assert!(validate_text(&[0xd800]).is_err());
        assert!(validate_text(&[]).is_err());
        assert!(validate_text(&"你好\nworld".encode_utf16().collect::<Vec<_>>()).is_ok());
    }
}
