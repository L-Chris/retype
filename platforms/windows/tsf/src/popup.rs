//! Compact, non-activating candidate surface. Native GDI keeps it independent of
//! a browser/runtime and allows the same TSF visibility contract as desktop hosts.
use crate::{edit, tip::TipState};
use retype_types::RenderState;
use std::sync::{Arc, Weak};
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::*,
    UI::{HiDpi::GetDpiForWindow, TextServices::ITfContext, WindowsAndMessaging::*},
};
use windows_core::{w, Result, PCWSTR};

struct Frame {
    render: RenderState,
    context: ITfContext,
    tip: Weak<TipState>,
    cells: Vec<RECT>,
    scale: i32,
    width: i32,
    height: i32,
}
fn px(value: i32, dpi: i32) -> i32 {
    (value * dpi + 48) / 96
}
unsafe fn font(size: i32, dpi: i32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -px(size, dpi),
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Microsoft YaHei UI"),
        )
    }
}
pub fn create(owner: Option<HWND>) -> Result<HWND> {
    // SAFETY: Register the class against this DLL (not the host executable).
    unsafe {
        let mut module = HMODULE::default();
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(window_proc as *const () as *const u16),
            &mut module,
        )?;
        let class = WNDCLASSW {
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(window_proc),
            hInstance: module.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("Retype.Candidates.1"),
            ..Default::default()
        };
        RegisterClassW(&class); // Already registered in this process is harmless.
        CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("Retype.Candidates.1"),
            w!("retype 候选"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            owner,
            None,
            Some(module.into()),
            None,
        )
    }
}
pub fn measure(render: &RenderState, window: Option<HWND>) -> (Vec<i32>, i32, i32) {
    // SAFETY: Temporary GDI objects are restored and released on this thread.
    unsafe {
        let dpi = window.map(|w| GetDpiForWindow(w)).unwrap_or(96).max(96) as i32;
        let dc = GetDC(window);
        let face = font(15, dpi, 400);
        let previous = SelectObject(dc, face.into());
        let widths = render
            .candidates
            .iter()
            .map(|candidate| {
                let mut size = SIZE::default();
                let _ = GetTextExtentPoint32W(
                    dc,
                    &candidate.text.encode_utf16().collect::<Vec<_>>(),
                    &mut size,
                );
                (size.cx + px(29, dpi)).max(px(43, dpi))
            })
            .collect();
        SelectObject(dc, previous);
        let _ = DeleteObject(face.into());
        ReleaseDC(window, dc);
        (widths, 480 - px(10, dpi), px(2, dpi))
    }
}

pub fn update(
    window: HWND,
    tip: &Arc<TipState>,
    context: &ITfContext,
    render: &RenderState,
    max_width: i32,
) -> (i32, i32) {
    // SAFETY: Our own HWND, only touched by its apartment; GDI resources restored before deletion.
    unsafe {
        let dpi = GetDpiForWindow(window).max(96) as i32;
        let dc = GetDC(Some(window));
        let face = font(15, dpi, 400);
        let previous = SelectObject(dc, face.into());
        let pad = px(5, dpi);
        let gap = px(2, dpi);
        let mut row = px(30, dpi);
        let viewport = max_width.clamp(100, 480);
        let visible = render.visible();
        let mut desired = Vec::with_capacity(visible.len());
        for candidate in visible {
            let mut size = SIZE::default();
            let _ = GetTextExtentPoint32W(
                dc,
                &candidate.text.encode_utf16().collect::<Vec<_>>(),
                &mut size,
            );
            desired.push((size.cx + px(29, dpi)).max(px(43, dpi)));
        }
        let available = viewport - pad * 2;
        let widths: Vec<i32> = desired.iter().map(|&width| width.min(available)).collect();
        // A single unusually long candidate wraps; ordinary phrases stay on one line.
        for (candidate, &width) in visible.iter().zip(&widths) {
            let mut rect = RECT {
                right: (width - px(24, dpi)).max(1),
                ..Default::default()
            };
            DrawTextW(
                dc,
                &mut candidate.text.encode_utf16().collect::<Vec<_>>(),
                &mut rect,
                DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
            );
            row = row.max(rect.bottom + px(10, dpi));
        }
        let mut x = pad;
        let mut cells = Vec::new();
        for width in widths {
            cells.push(RECT {
                left: x,
                top: pad,
                right: x + width,
                bottom: pad + row,
            });
            x += width + gap;
        }
        let width = if cells.is_empty() {
            px(60, dpi)
        } else {
            x - gap + pad
        };
        let height = row + pad * 2;
        SelectObject(dc, previous);
        let _ = DeleteObject(face.into());
        ReleaseDC(Some(window), dc);
        let frame = Box::new(Frame {
            render: render.clone(),
            context: context.clone(),
            tip: Arc::downgrade(tip),
            cells,
            scale: dpi,
            width,
            height,
        });
        let old = SetWindowLongPtrW(window, GWLP_USERDATA, Box::into_raw(frame) as _);
        if old != 0 {
            drop(Box::from_raw(old as *mut Frame));
        }
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, px(10, dpi), px(10, dpi));
        if SetWindowRgn(window, Some(region), false) == 0 {
            let _ = DeleteObject(region.into());
        }
        // Window text provides a fallback accessible name alongside TSF candidate enumeration.
        let label = format!(
            "{} · {}",
            render.composition,
            render
                .visible()
                .iter()
                .enumerate()
                .map(|(i, c)| format!("{} {}", i + 1, c.text))
                .collect::<Vec<_>>()
                .join("  ")
        );
        let label: Vec<u16> = label.encode_utf16().chain([0]).collect();
        let _ = SetWindowTextW(window, PCWSTR(label.as_ptr()));
        let _ = InvalidateRect(Some(window), None, false);
        (width, height)
    }
}

unsafe fn rounded(dc: HDC, rect: RECT, fill: u32, border: u32, radius: i32) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(fill));
        let pen = CreatePen(PS_SOLID, 1, COLORREF(border));
        let old_brush = SelectObject(dc, brush.into());
        let old_pen = SelectObject(dc, pen.into());
        let _ = RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius,
            radius,
        );
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        let _ = DeleteObject(pen.into());
        let _ = DeleteObject(brush.into());
    }
}
unsafe fn text(dc: HDC, value: &str, mut rect: RECT, color: u32, flags: DRAW_TEXT_FORMAT) {
    unsafe {
        SetTextColor(dc, COLORREF(color));
        DrawTextW(
            dc,
            &mut value.encode_utf16().collect::<Vec<_>>(),
            &mut rect,
            DT_NOPREFIX | flags,
        );
    }
}
unsafe fn paint(window: HWND, frame: &Frame) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let target = BeginPaint(window, &mut ps);
        let dc = CreateCompatibleDC(Some(target));
        let bitmap = CreateCompatibleBitmap(target, frame.width, frame.height);
        let old_bitmap = SelectObject(dc, bitmap.into());
        let bounds = RECT {
            left: 0,
            top: 0,
            right: frame.width,
            bottom: frame.height,
        };
        let bg = CreateSolidBrush(COLORREF(0x00fdfcfc));
        FillRect(dc, &bounds, bg);
        let _ = DeleteObject(bg.into());
        rounded(dc, bounds, 0x00fdfcfc, 0x00e6e2df, px(10, frame.scale));
        SetBkMode(dc, TRANSPARENT);
        let small = font(11, frame.scale, 400);
        let main = font(15, frame.scale, 400);
        let old_font = SelectObject(dc, small.into());
        let p = |v| px(v, frame.scale);
        for (i, (candidate, cell)) in frame.render.visible().iter().zip(&frame.cells).enumerate() {
            let selected = frame.render.page_start + i == frame.render.selected;
            if selected {
                rounded(dc, *cell, 0x00968f13, 0x00968f13, p(8));
            }
            SelectObject(dc, small.into());
            text(
                dc,
                &(i + 1).to_string(),
                RECT {
                    left: cell.left + p(6),
                    right: cell.left + p(19),
                    ..*cell
                },
                if selected { 0x00ffffff } else { 0x009a938e },
                DT_LEFT | DT_SINGLELINE | DT_VCENTER,
            );
            SelectObject(dc, main.into());
            text(
                dc,
                &candidate.text,
                RECT {
                    top: cell.top + p(5),
                    left: cell.left + p(19),
                    right: cell.right - p(5),
                    ..*cell
                },
                if selected { 0x00ffffff } else { 0x00443d34 },
                DT_LEFT,
            );
        }
        SelectObject(dc, old_font);
        let _ = BitBlt(
            target,
            0,
            0,
            frame.width,
            frame.height,
            Some(dc),
            0,
            0,
            SRCCOPY,
        );
        SelectObject(dc, old_bitmap);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        let _ = DeleteObject(small.into());
        let _ = DeleteObject(main.into());
        let _ = EndPaint(window, &ps);
    }
}
unsafe extern "system" fn window_proc(window: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: USERDATA is exclusively our Box<Frame>. Copy click targets before any
    // reentrant TSF call (which may replace/free that frame). No panic crosses Win32.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let pointer = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut Frame;
        if msg == WM_NCDESTROY {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            if !pointer.is_null() {
                drop(Box::from_raw(pointer));
            }
        } else if msg == WM_MOUSEACTIVATE {
            return LRESULT(MA_NOACTIVATE as isize);
        } else if msg == WM_ERASEBKGND {
            return LRESULT(1);
        } else if !pointer.is_null() {
            if msg == WM_PAINT {
                paint(window, &*pointer);
                return LRESULT(0);
            }
            if msg == WM_LBUTTONUP {
                let frame = &*pointer;
                let x = (lp.0 & 0xffff) as i16 as i32;
                let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
                let target = frame
                    .cells
                    .iter()
                    .position(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
                    .map(|i| {
                        (
                            Weak::clone(&frame.tip),
                            frame.context.clone(),
                            frame.render.page_start + i,
                            frame.render.gen,
                        )
                    });
                if let Some((tip, context, index, generation)) = target {
                    if let Some(tip) = tip.upgrade() {
                        let _ =
                            edit::request(&tip, &context, edit::Work::Choose(index, generation));
                    }
                }
                return LRESULT(0);
            }
        }
        DefWindowProcW(window, msg, wp, lp)
    }))
    .unwrap_or(LRESULT(0))
}
