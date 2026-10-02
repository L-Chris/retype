//! Full-context translation. COM objects stay on the host's TSF apartment.
use crate::tip::{guarded, lock, TipState};
use retype_ai::protocol::{Operation, Response, MAX_TEXT};
use std::{
    mem::ManuallyDrop,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Weak,
    },
};
use windows::{
    core::{implement, w, Interface, Result, PCWSTR},
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{DataExchange::*, LibraryLoader::*, Memory::*},
        UI::{HiDpi::GetDpiForWindow, TextServices::*, WindowsAndMessaging::*},
    },
};
const TIMER: usize = 88;
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
        Ok(text) => (text, "正在翻译…".to_owned()),
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
    if !crate::edit::learnable(context, ec) {
        return Err("此输入框不允许读取全文".into());
    }
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
        return Err("原文已变化，未替换；可复制译文".into());
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
        start
            .SetText(ec, 0, &units)
            .map_err(|_| "应用拒绝替换；可复制译文")?;
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
        status(hwnd, "输入框已切换或正在输入，原文保持不变；可复制译文");
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
    if !result.is_ok_and(|s| s.is_ok()) {
        status(hwnd, "应用拒绝替换，原文保持不变；可复制译文");
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
                status(hwnd, "输入框已切换，原文保持不变；可复制译文");
                return Ok(());
            }
            let result = replace_checked(&self.context, ec, &self.original, &self.text, || {
                !self.cancelled.load(Ordering::Relaxed)
                    && same_target(&state, &self.context, self.epoch, self.owner)
            });
            if result.is_ok() {
                status(hwnd, "翻译完成");
                unsafe {
                    SetTimer(Some(hwnd), 99, 1400, None);
                }
            } else if let Err(error) = result {
                status(hwnd, &error);
            }
            // Translation is not typing activity and does not enter personal word learning.
            Ok(())
        })
    }
}
unsafe fn frame<'a>(hwnd: HWND) -> Option<&'a mut Frame> {
    // SAFETY: GWLP_USERDATA is our exclusive Box<Frame>, accessed only on its window thread.
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Frame;
    unsafe { pointer.as_mut() }
}
fn status(hwnd: HWND, message: &str) {
    // SAFETY: called only on the window owner thread, outside any existing frame borrow.
    let expand = unsafe {
        if let Some(f) = frame(hwnd) {
            f.message = message.into();
            f.applying = false;
            if message == "翻译完成" {
                f.applied = true;
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            f.translation.is_some() && !f.applied
        } else {
            false
        }
    };
    if expand {
        resize_preview(hwnd);
    }
}
fn resize_preview(hwnd: HWND) {
    // SAFETY: resize the non-activating owned window within its monitor work area.
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return;
        }
        let height = 300 * GetDpiForWindow(hwnd).max(96) as i32 / 96;
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
        let _ = SetWindowPos(
            hwnd,
            None,
            rect.left,
            top,
            rect.right - rect.left,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}
fn create(data: Frame, context: &ITfContext, ec: u32) -> Result<HWND> {
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
        let width = 480 * dpi / 96;
        let height = 120 * dpi / 96;
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
        let pointer = Box::into_raw(Box::new(data));
        #[cfg(target_pointer_width = "64")]
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as isize);
        #[cfg(target_pointer_width = "32")]
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as i32);
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
        SetTimer(Some(hwnd), TIMER, 100, None);
        Ok(hwnd)
    }
}
unsafe fn paint(hwnd: HWND) {
    // SAFETY: paint lifecycle and temporary font/brush handles are paired and restored.
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let brush = CreateSolidBrush(COLORREF(0x00faf9f6));
        FillRect(dc, &rect, brush);
        let _ = DeleteObject(brush.into());
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        let pad = 16 * dpi / 96;
        let bar = 44 * dpi / 96;
        let font = CreateFontW(
            -16 * dpi / 96,
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
        );
        let old = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x003b2b14));
        if let Some(f) = frame(hwnd) {
            let mut heading = RECT {
                left: pad,
                top: pad,
                right: rect.right - pad,
                bottom: pad + bar,
            };
            let mut message: Vec<u16> = f.message.encode_utf16().collect();
            DrawTextW(dc, &mut message, &mut heading, DT_WORDBREAK | DT_NOPREFIX);
            let top = pad + bar + 8 * dpi / 96;
            let bottom = rect.bottom - bar - pad;
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
            DrawTextW(
                dc,
                &mut text,
                &mut measured,
                DT_WORDBREAK | DT_NOPREFIX | DT_CALCRECT,
            );
            f.max_scroll = (measured.bottom - (bottom - top)).max(0);
            DrawTextW(dc, &mut text, &mut body, DT_WORDBREAK | DT_NOPREFIX);
            let _ = RestoreDC(dc, saved);
            for (i, label) in ["替换", "复制译文", "关闭"].iter().enumerate() {
                let start = pad + i as i32 * (rect.right - pad * 2) / 3;
                let mut button = RECT {
                    left: start,
                    top: rect.bottom - bar,
                    right: start + (rect.right - pad * 2) / 3 - 6,
                    bottom: rect.bottom - 8 * dpi / 96,
                };
                DrawTextW(
                    dc,
                    &mut label.encode_utf16().collect::<Vec<_>>(),
                    &mut button,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
            }
        }
        SelectObject(dc, old);
        let _ = DeleteObject(font.into());
        let _ = EndPaint(hwnd, &ps);
    }
}
fn clipboard(hwnd: HWND, text: &str) -> Result<()> {
    // SAFETY: clipboard owns the memory only after successful SetClipboardData.
    unsafe {
        OpenClipboard(Some(hwnd))?;
        let result = (|| {
            EmptyClipboard()?;
            let units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2)?;
            let pointer = GlobalLock(memory);
            if pointer.is_null() {
                let _ = GlobalFree(Some(memory));
                return Err(E_FAIL.into());
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), pointer.cast::<u16>(), units.len());
            let _ = GlobalUnlock(memory);
            if let Err(error) = SetClipboardData(13, Some(HANDLE(memory.0))) {
                let _ = GlobalFree(Some(memory));
                return Err(error);
            }
            Ok(())
        })();
        let _ = CloseClipboard();
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
                WM_TIMER if wp.0 == 99 => {
                    let _ = DestroyWindow(hwnd);
                    LRESULT(0)
                }
                WM_TIMER => {
                    let response = frame(hwnd).and_then(|f| f.response.as_ref()?.try_recv().ok());
                    if let Some(response) = response {
                        let apply = if let Some(f) = frame(hwnd) {
                            f.response = None;
                            match response {
                                Ok(Response::Translation { text, preview }) => {
                                    f.translation = Some(text);
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
                        if !apply && frame(hwnd).is_some_and(|f| f.translation.is_some()) {
                            resize_preview(hwnd);
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
                    if y > rect.bottom - 44 * GetDpiForWindow(hwnd).max(96) as i32 / 96 {
                        if x < rect.right / 3 {
                            request_apply(hwnd);
                        } else if x < rect.right * 2 / 3 {
                            let text = frame(hwnd).and_then(|f| f.translation.clone());
                            if let Some(text) = text {
                                if clipboard(hwnd, &text).is_ok() {
                                    status(hwnd, "译文已复制");
                                }
                            }
                        } else {
                            let _ = DestroyWindow(hwnd);
                        }
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
