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

pub fn acquire(updates: bool) -> Result<Option<Guard>> {
    let path = std::env::current_exe()?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().to_lowercase().hash(&mut hash);
    let name = wide(&format!("Local\\Retype.Settings.{:016x}", hash.finish()));
    // SAFETY: Null-terminated mutex name. Only our owned handle is closed.
    unsafe {
        let mutex = CreateMutexW(None, false, PCWSTR(name.as_ptr()))?;
        let existing = GetLastError() == ERROR_ALREADY_EXISTS;
        let guard = Guard(mutex);
        if !existing {
            return Ok(Some(guard));
        }
        for _ in 0..100 {
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
                    let _ = AllowSetForegroundWindow(pid);
                    PostMessageW(Some(window), SHOW, WPARAM(0), LPARAM(isize::from(updates)))?;
                    return Ok(None);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err("已有设置程序尚未就绪，请稍后重试".into())
    }
}
struct State {
    ctx: egui::Context,
    main: HWND,
    requests: Arc<Mutex<Vec<bool>>>,
    busy: Arc<AtomicBool>,
}
pub struct Window {
    handle: HWND,
    pub requests: Arc<Mutex<Vec<bool>>>,
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
            Ok(Self { handle, requests })
        }
    }
    pub fn idle_timer(&self, delay: Duration) {
        // SAFETY: Timer belongs to this owned message window on the UI thread.
        unsafe {
            SetTimer(
                Some(self.handle),
                1,
                delay.as_millis().clamp(1, u32::MAX as u128) as u32,
                None,
            );
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
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            drop(Box::from_raw(state));
        } else if message == SHOW && !state.is_null() {
            let _ = KillTimer(Some(window), 1);
            let state = &*state;
            state
                .requests
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(lp.0 != 0);
            let _ = ShowWindow(state.main, SW_RESTORE);
            let _ = SetForegroundWindow(state.main);
            state.ctx.request_repaint();
            return LRESULT(0);
        } else if message == WM_TIMER && !state.is_null() {
            let state = &*state;
            if state.busy.load(Ordering::Relaxed) {
                SetTimer(Some(window), 1, 1000, None);
            } else {
                let _ = KillTimer(Some(window), 1);
                // Hidden eframe windows may defer CloseRequested until their next
                // paint. Preferences are saved eagerly and no worker is active,
                // so end the retained process without making the window visible.
                std::process::exit(0);
            }
            return LRESULT(0);
        }
        DefWindowProcW(window, message, wp, lp)
    }))
    .unwrap_or(LRESULT(0))
}
