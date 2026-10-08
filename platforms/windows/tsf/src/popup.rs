//! Non-activating Win32 surface with egui layout and on-demand CPU rendering.
use crate::{edit, tip::TipState};
use retype_candidate_ui::surface::{Bitmap, Surface};
use retype_types::RenderState;
use std::cell::RefCell;

thread_local! { static SURFACE: RefCell<Surface> = RefCell::new(Surface::default()); }
use std::sync::{Arc, Weak};
use std::time::Instant;
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::*,
    UI::{HiDpi::GetDpiForWindow, TextServices::ITfContext, WindowsAndMessaging::*},
};
use windows_core::{w, Result, PCWSTR};

struct Frame {
    generation: u64,
    page_start: usize,
    composition: String,
    texts: Vec<String>,
    context: ITfContext,
    tip: Weak<TipState>,
    bitmap: Arc<Bitmap>,
    width: i32,
    height: i32,
    dpi: i32,
    invalidated: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct UpdateMetrics {
    pub notify_us: u64,
    pub notify_result: i32,
    pub getter_calls: u64,
    pub string_calls: u64,
    pub updated_flags: u32,
    pub raster_us: u64,
    pub reused: bool,
    pub dpi: i32,
}

pub(crate) struct Update {
    pub width: i32,
    pub height: i32,
    pub metrics: UpdateMetrics,
}

/// One set of physical pixel measurements for pagination and popup drawing.
pub struct Layout {
    pub widths: Vec<i32>,
    pub available: i32,
    pub gap: i32,
    dpi: i32,
    viewport: i32,
}
fn px(value: i32, dpi: i32) -> i32 {
    (value * dpi + 48) / 96
}
fn viewport_width(work_area_width: i32, dpi: i32) -> i32 {
    work_area_width.clamp(px(100, dpi), px(480, dpi))
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
pub fn measure(render: &RenderState, window: Option<HWND>, anchor: Option<RECT>) -> Layout {
    // SAFETY: Read-only Win32 DPI/monitor queries; egui stays on this apartment.
    unsafe {
        let dpi = window.map(|w| GetDpiForWindow(w)).unwrap_or(96).max(96) as i32;
        let max_width = anchor
            .and_then(|rect| {
                let monitor = MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST);
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                GetMonitorInfoW(monitor, &mut info)
                    .as_bool()
                    .then_some(info.rcWork.right - info.rcWork.left - px(16, dpi))
            })
            .unwrap_or_else(|| px(480, dpi));
        let viewport = viewport_width(max_width, dpi);
        let pad = px(4, dpi);
        let available = viewport - 2 * pad;
        let gap = px(2, dpi);
        let widths = SURFACE.with(|surface| {
            surface
                .borrow_mut()
                .measure(render, available, dpi as f32 / 96.0)
        });
        Layout {
            widths,
            available,
            gap,
            dpi,
            viewport,
        }
    }
}

pub fn update(
    window: HWND,
    tip: &Arc<TipState>,
    context: &ITfContext,
    render: &RenderState,
    layout: &Layout,
) -> Update {
    // SAFETY: Our own HWND, only touched by its apartment; bitmap owned by Frame.
    unsafe {
        let dpi = layout.dpi;
        let visible = render.visible();
        let widths = layout
            .widths
            .get(render.page_start..render.page_start + visible.len())
            .unwrap_or(&[]);
        let start = Instant::now();
        let bitmap = SURFACE.with(|surface| {
            surface.borrow_mut().render(
                render,
                widths,
                layout.gap,
                layout.viewport,
                dpi as f32 / 96.0,
            )
        });
        let raster_us = start.elapsed().as_micros().min(u64::MAX as u128) as u64;
        let width = bitmap.geometry.width as i32;
        let height = bitmap.geometry.height as i32;
        let old = GetWindowLongPtrW(window, GWLP_USERDATA) as *const Frame;
        let reused = !old.is_null() && Arc::ptr_eq(&(*old).bitmap, &bitmap);
        let geometry_changed =
            old.is_null() || (*old).width != width || (*old).height != height || (*old).dpi != dpi;
        let label_changed = old.is_null()
            || (*old).composition != render.composition
            || !(*old)
                .texts
                .iter()
                .map(String::as_str)
                .eq(visible.iter().map(|c| c.text.as_str()));
        let invalidated = if reused {
            (*old).invalidated
        } else {
            Some(Instant::now())
        };
        let frame = Box::new(Frame {
            generation: render.gen,
            page_start: render.page_start,
            composition: render.composition.clone(),
            texts: visible.iter().map(|c| c.text.clone()).collect(),
            context: context.clone(),
            tip: Arc::downgrade(tip),
            bitmap,
            width,
            height,
            dpi,
            invalidated,
        });
        let old = SetWindowLongPtrW(window, GWLP_USERDATA, Box::into_raw(frame) as _);
        if old != 0 {
            drop(Box::from_raw(old as *mut Frame));
        }
        if geometry_changed {
            let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, px(10, dpi), px(10, dpi));
            if SetWindowRgn(window, Some(region), false) == 0 {
                let _ = DeleteObject(region.into());
            }
        }
        // Window text provides a fallback accessible name alongside TSF candidate enumeration.
        if label_changed {
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
        }
        if !reused {
            let _ = InvalidateRect(Some(window), None, false);
        }
        Update {
            width,
            height,
            metrics: UpdateMetrics {
                raster_us,
                reused,
                dpi,
                ..Default::default()
            },
        }
    }
}

unsafe fn paint(window: HWND, frame: &Frame) {
    // SAFETY: The owned bitmap has exactly width*height top-down BGRX pixels.
    // GDI only presents the completed egui frame; it does not measure/draw text.
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let target = BeginPaint(window, &mut ps);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: frame.width,
                biHeight: -frame.height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        StretchDIBits(
            target,
            0,
            0,
            frame.width,
            frame.height,
            0,
            0,
            frame.width,
            frame.height,
            Some(frame.bitmap.pixels.as_ptr().cast()),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
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
                let invalidated = (*pointer).invalidated.take();
                let generation = (*pointer).generation;
                let dpi = (*pointer).dpi;
                let start = Instant::now();
                paint(window, &*pointer);
                if let Some(queued) = invalidated {
                    crate::input_profile::paint(generation, queued, start, dpi);
                }
                return LRESULT(0);
            }
            if msg == WM_LBUTTONUP {
                let frame = &*pointer;
                let x = (lp.0 & 0xffff) as i16 as i32;
                let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
                let target = frame.bitmap.geometry.hit_test(x as f32, y as f32).map(|i| {
                    (
                        Weak::clone(&frame.tip),
                        frame.context.clone(),
                        frame.page_start + i,
                        frame.generation,
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

#[cfg(test)]
pub(crate) fn frame_for_test(window: HWND) -> (Arc<Bitmap>, u64) {
    // SAFETY: Integration test owns this popup on its TSF apartment.
    unsafe {
        let frame = &*(GetWindowLongPtrW(window, GWLP_USERDATA) as *const Frame);
        (Arc::clone(&frame.bitmap), frame.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use retype_types::{Candidate, CandidateSource};

    #[test]
    fn eight_short_phrases_fit_within_the_480_pixel_limit() {
        let render = RenderState {
            candidates: (0..8)
                .map(|_| Candidate::new("你好", CandidateSource::Local))
                .collect(),
            ..Default::default()
        };
        let layout = measure(&render, None, None);
        let required = layout.widths.iter().sum::<i32>() + layout.gap * 7;
        assert!(required <= layout.available);
        assert_eq!(layout.viewport, 480);
    }

    #[test]
    fn viewport_limit_scales_with_dpi_but_stays_within_the_monitor() {
        assert_eq!(viewport_width(1920, 96), 480);
        assert_eq!(viewport_width(1920, 144), 720);
        assert_eq!(viewport_width(600, 144), 600);
    }

    #[test]
    fn candidate_window_retains_nonactivating_native_contract() {
        // SAFETY: Test owns the window and destroys it on its creating thread.
        unsafe {
            let foreground = GetForegroundWindow();
            let window = create(None).unwrap_or_default();
            assert!(!window.is_invalid());
            let style = GetWindowLongW(window, GWL_EXSTYLE) as u32;
            assert_ne!(style & WS_EX_NOACTIVATE.0, 0);
            assert_ne!(style & WS_EX_TOOLWINDOW.0, 0);
            assert_ne!(style & WS_EX_TOPMOST.0, 0);
            assert_eq!(
                window_proc(window, WM_MOUSEACTIVATE, WPARAM(0), LPARAM(0)),
                LRESULT(MA_NOACTIVATE as isize)
            );
            let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
            assert_eq!(GetForegroundWindow(), foreground);
            assert!(DestroyWindow(window).is_ok());
        }
    }
}
