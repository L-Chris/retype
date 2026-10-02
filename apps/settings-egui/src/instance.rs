//! Same-install single-instance routing. The message window remains discoverable
//! while the eframe window is hidden; old install directories have separate locks.
#![allow(unsafe_code)]
use crate::backend::{wide, Result};
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    hash::{Hash, Hasher},
    sync::{Arc, Mutex},
    time::Duration,
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::*,
        System::{LibraryLoader::*, Threading::*},
        UI::WindowsAndMessaging::*,
    },
};

pub const SHOW: u32 = WM_APP + 29;
pub struct Guard(HANDLE);
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn acquire(updates: bool, request: u32) -> Result<Option<Guard>> {
    crate::settings_log::event("app", "mutex_begin", request, "same-install instance check");
    let path = std::env::current_exe()?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().to_lowercase().hash(&mut hash);
    let name = wide(&format!("Local\\Retype.Settings.{:016x}", hash.finish()));
    // SAFETY: Null-terminated mutex name. Only our owned handle is closed.
    unsafe {
        let mutex = CreateMutexW(None, false, PCWSTR(name.as_ptr()))?;
        let existing = GetLastError() == ERROR_ALREADY_EXISTS;
        crate::settings_log::event(
            "app",
            "mutex_ready",
            request,
            format!("existing={existing}"),
        );
        let guard = Guard(mutex);
        if !existing {
            return Ok(Some(guard));
        }
        let started = std::time::Instant::now();
        for attempt in 0..100 {
            let mut previous = None;
            while let Ok(window) = FindWindowExW(
                None,
                previous,
                w!("Retype.Settings.Command"),
                w!("retype 设置"),
            ) {
                previous = Some(window);
                let mut pid = 0;
                GetWindowThreadProcessId(window, Some(&mut pid));
                let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                    if attempt == 0 {
                        crate::settings_log::event(
                            "app",
                            "reuse_query_denied",
                            request,
                            format!("target_pid={pid} win32={}", GetLastError().0),
                        );
                    }
                    continue;
                };
                let mut buffer = vec![0u16; 32768];
                let mut length = buffer.len() as u32;
                let found = QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_WIN32,
                    windows::core::PWSTR(buffer.as_mut_ptr()),
                    &mut length,
                )
                .is_ok();
                let _ = CloseHandle(process);
                if found
                    && String::from_utf16_lossy(&buffer[..length as usize])
                        .eq_ignore_ascii_case(&path.to_string_lossy())
                {
                    let allowed = AllowSetForegroundWindow(pid);
                    crate::settings_log::event(
                        "app",
                        "reuse_target",
                        request,
                        format!(
                            "target_pid={pid} foreground_allowed={} elapsed_ms={}",
                            allowed.is_ok(),
                            started.elapsed().as_millis()
                        ),
                    );
                    let posted = PostMessageW(
                        Some(window),
                        SHOW,
                        WPARAM(request as usize),
                        LPARAM(isize::from(updates)),
                    );
                    crate::settings_log::event(
                        "app",
                        "reuse_post",
                        request,
                        format!(
                            "target_pid={pid} success={} hresult={}",
                            posted.is_ok(),
                            posted.as_ref().err().map_or(0, |e| e.code().0)
                        ),
                    );
                    posted?;
                    return Ok(None);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        crate::settings_log::event(
            "app",
            "reuse_timeout",
            request,
            format!("elapsed_ms={}", started.elapsed().as_millis()),
        );
        Err("已有设置程序尚未就绪，请稍后重试".into())
    }
}
struct State {
    ctx: egui::Context,
    main: HWND,
    requests: Arc<Mutex<Vec<ShowRequest>>>,
    busy: Arc<AtomicBool>,
}
pub struct Window {
    handle: HWND,
    pub requests: Arc<Mutex<Vec<ShowRequest>>>,
}
pub struct ShowRequest {
    pub updates: bool,
    pub id: u32,
    pub received: std::time::Instant,
}
impl Window {
    pub fn create(main: HWND, ctx: egui::Context, busy: Arc<AtomicBool>) -> Result<Self> {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = Box::new(State {
            ctx,
            main,
            requests: Arc::clone(&requests),
            busy,
        });
        // SAFETY: Window and state are confined to the eframe UI thread.
        unsafe {
            let module = GetModuleHandleW(None)?;
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: module.into(),
                lpszClassName: w!("Retype.Settings.Command"),
                ..Default::default()
            });
            let handle = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("Retype.Settings.Command"),
                w!("retype 设置"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(module.into()),
                None,
            )?;
            SetWindowLongPtrW(handle, GWLP_USERDATA, Box::into_raw(state) as isize);
            crate::settings_log::event(
                "app",
                "command_window_ready",
                0,
                format!("hwnd={}", handle.0 as usize),
            );
            Ok(Self { handle, requests })
        }
    }
    pub fn idle_timer(&self, delay: Duration) {
        // SAFETY: Timer belongs to this owned message window on the UI thread.
        unsafe {
            let timer = SetTimer(
                Some(self.handle),
                1,
                delay.as_millis().clamp(1, u32::MAX as u128) as u32,
                None,
            );
            crate::settings_log::event(
                "app",
                "idle_timer",
                0,
                format!("delay_ms={} timer={timer}", delay.as_millis()),
            );
        }
    }
    pub fn log_visible(&self, request: u32) {
        // SAFETY: only inspecting the main window owned by this UI apartment.
        unsafe {
            let state = GetWindowLongPtrW(self.handle, GWLP_USERDATA) as *const State;
            if let Some(state) = state.as_ref() {
                log_window(state.main, request);
            }
        }
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.handle);
        }
    }
}
unsafe extern "system" fn proc(window: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let state = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut State;
        if message == WM_NCDESTROY && !state.is_null() {
            crate::settings_log::event(
                "app",
                "command_window_destroyed",
                0,
                "instance endpoint removed",
            );
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            drop(Box::from_raw(state));
        } else if message == SHOW && !state.is_null() {
            let request = wp.0 as u32;
            crate::settings_log::event(
                "app",
                "show_received",
                request,
                format!("updates={}", lp.0 != 0),
            );
            let _ = KillTimer(Some(window), 1);
            let state = &*state;
            state
                .requests
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(ShowRequest {
                    updates: lp.0 != 0,
                    id: request,
                    received: std::time::Instant::now(),
                });
            let _ = ShowWindow(state.main, SW_RESTORE);
            let foreground = SetForegroundWindow(state.main);
            crate::settings_log::event(
                "app",
                "show_native",
                request,
                format!("foreground_result={}", foreground.as_bool()),
            );
            log_window(state.main, request);
            state.ctx.request_repaint();
            return LRESULT(0);
        } else if message == WM_TIMER && !state.is_null() {
            let state = &*state;
            if state.busy.load(Ordering::Relaxed) {
                crate::settings_log::event(
                    "app",
                    "idle_exit_deferred",
                    0,
                    "background worker busy",
                );
                SetTimer(Some(window), 1, 1000, None);
            } else {
                let _ = KillTimer(Some(window), 1);
                // Hidden eframe windows may defer CloseRequested until their next
                // paint. Preferences are saved eagerly and no worker is active,
                // so end the retained process without making the window visible.
                crate::settings_log::event("app", "idle_exit", 0, "hidden instance shutdown");
                crate::settings_log::flush();
                std::process::exit(0);
            }
            return LRESULT(0);
        }
        DefWindowProcW(window, message, wp, lp)
    }))
    .unwrap_or(LRESULT(0))
}
unsafe fn log_window(window: HWND, request: u32) {
    // SAFETY: read-only state of our main window, never the user's document window.
    unsafe {
        let mut rect = RECT::default();
        let result = GetWindowRect(window, &mut rect);
        crate::settings_log::event(
            "app",
            "window_state",
            request,
            format!(
                "visible={} iconic={} foreground={} rect_ok={} rect={},{},{},{}",
                IsWindowVisible(window).as_bool(),
                IsIconic(window).as_bool(),
                GetForegroundWindow() == window,
                result.is_ok(),
                rect.left,
                rect.top,
                rect.right,
                rect.bottom
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forwarded_show_preserves_request_id_without_opening_a_desktop_window() -> Result<()> {
        // Message-only windows exercise routing without activating or typing into any app.
        let thread = std::thread::spawn(|| -> Result<()> {
            unsafe {
                let parent = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("retype diagnostics test"),
                    WINDOW_STYLE::default(),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    None,
                    None,
                )?;
                let outcome = (|| -> Result<()> {
                    let context = egui::Context::default();
                    let endpoint =
                        Window::create(parent, context, Arc::new(AtomicBool::new(false)))?;
                    let request = crate::settings_log::request_id();
                    PostMessageW(
                        Some(endpoint.handle),
                        SHOW,
                        WPARAM(request as usize),
                        LPARAM(1),
                    )?;
                    let mut message = MSG::default();
                    while PeekMessageW(&mut message, Some(endpoint.handle), 0, 0, PM_REMOVE)
                        .as_bool()
                    {
                        DispatchMessageW(&message);
                    }
                    let received = endpoint.requests.lock().unwrap_or_else(|e| e.into_inner());
                    assert_eq!(received.len(), 1);
                    assert_eq!(received[0].id, request);
                    assert!(received[0].updates);
                    Ok(())
                })();
                let _ = DestroyWindow(parent);
                outcome
            }
        });
        thread
            .join()
            .map_err(|_| "diagnostic routing test thread failed")?
    }
}
