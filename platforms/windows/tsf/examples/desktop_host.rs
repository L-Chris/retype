//! Standalone RichEdit desktop host for manual IME acceptance, without opening user documents.
#![windows_subsystem = "windows"]
#![allow(unsafe_code)]
use std::cell::RefCell;
use windows::Win32::{
    Foundation::*,
    System::{Com::*, LibraryLoader::*},
    UI::{TextServices::*, WindowsAndMessaging::*},
};
use windows_core::{w, Interface, Result, PCWSTR};
thread_local! {
    static MODE_ITEM: RefCell<Option<ITfLangBarItemButton>> = const { RefCell::new(None) };
    static MODE_LABEL: RefCell<String> = const { RefCell::new(String::new()) };
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: Standard Win32 window procedure; the child is identified by its control ID.
    unsafe {
        if msg == WM_COMMAND {
            let command = wp.0 as u32 & 0xffff;
            if (101..=103).contains(&command) {
                let item = MODE_ITEM.with(|i| i.borrow().clone());
                if let Some(item) = item {
                    let _ = item.OnMenuSelect(command - 100);
                }
                if let Ok(edit) = GetDlgItem(Some(hwnd), 1) {
                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(edit));
                }
                return LRESULT(0);
            }
        }
        if msg == WM_TIMER {
            let item = MODE_ITEM.with(|i| i.borrow().clone());
            if let Some(item) = item {
                let label = item
                    .GetTooltipString()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let changed = MODE_LABEL.with(|old| {
                    if *old.borrow() == label {
                        false
                    } else {
                        *old.borrow_mut() = label.clone();
                        true
                    }
                });
                if changed {
                    let title: Vec<u16> = format!(
                        "retype M1 — isolated RichEdit test | {}",
                        label.lines().next().unwrap_or("")
                    )
                    .encode_utf16()
                    .chain([0])
                    .collect();
                    let _ = SetWindowTextW(hwnd, PCWSTR(title.as_ptr()));
                    if let Ok(icon) = item.GetIcon() {
                        let old = SendMessageW(
                            hwnd,
                            WM_SETICON,
                            Some(WPARAM(ICON_SMALL as usize)),
                            Some(LPARAM(icon.0 as isize)),
                        );
                        if old.0 != 0 {
                            let _ = DestroyIcon(HICON(old.0 as *mut _));
                        }
                    }
                }
            }
            return LRESULT(0);
        }
        if msg == WM_SIZE {
            if let Ok(child) = GetDlgItem(Some(hwnd), 1) {
                let _ = MoveWindow(
                    child,
                    12,
                    12,
                    (lp.0 & 0xffff) as i32 - 24,
                    ((lp.0 >> 16) & 0xffff) as i32 - 24,
                    true,
                );
            }
        }
        if msg == WM_DESTROY {
            let _ = KillTimer(Some(hwnd), 1);
            let icon = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(0)),
            );
            if icon.0 != 0 {
                let _ = DestroyIcon(HICON(icon.0 as *mut _));
            }
            MODE_ITEM.with(|i| {
                i.borrow_mut().take();
            });
            PostQuitMessage(0);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

#[allow(clippy::manual_dangling_ptr)] // HMENU encodes child control ID 1, not an address.
fn main() -> Result<()> {
    // SAFETY: All UI and COM work stays on this single apartment thread.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let module = GetModuleHandleW(None)?;
        let _rich_edit = LoadLibraryW(w!("Msftedit.dll"))?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: module.into(),
            lpszClassName: w!("RetypeM1Host"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let window = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("RetypeM1Host"),
            w!("retype M1 — isolated RichEdit test"),
            WS_OVERLAPPEDWINDOW,
            150,
            150,
            900,
            500,
            None,
            None,
            Some(module.into()),
            None,
        )?;
        let edit = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("RICHEDIT50W"),
            w!(""),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WS_VSCROLL
                | WINDOW_STYLE(ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32),
            12,
            12,
            850,
            420,
            Some(window),
            Some(HMENU(1usize as *mut _)),
            Some(module.into()),
            None,
        )?;
        let manager: ITfThreadMgr =
            CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)?;
        let _tid = manager.Activate()?;
        let profiles: ITfInputProcessorProfileMgr =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        profiles.ActivateProfile(
            TF_PROFILETYPE_INPUTPROCESSOR,
            retype_ime::ids::LANGID_ZH_CN,
            &retype_ime::ids::CLSID_RETYPE_TIP,
            &retype_ime::ids::GUID_PROFILE_RETYPE,
            windows::Win32::UI::Input::KeyboardAndMouse::HKL::default(),
            TF_IPPMF_FORPROCESS,
        )?;
        let items: ITfLangBarItemMgr = manager.cast()?;
        let item: ITfLangBarItemButton = items.GetItem(&GUID_LBI_INPUTMODE)?.cast()?;
        MODE_ITEM.with(|value| *value.borrow_mut() = Some(item));
        let menu = CreateMenu()?;
        AppendMenuW(menu, MF_STRING, 101, w!("中 / 英"))?;
        AppendMenuW(menu, MF_STRING, 102, w!("全拼"))?;
        AppendMenuW(menu, MF_STRING, 103, w!("小鹤双拼"))?;
        SetMenu(window, Some(menu))?;
        SetTimer(Some(window), 1, 250, None);
        let _ = ShowWindow(window, SW_SHOW);
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(edit));
        let mut message = MSG::default();
        let keys: ITfKeystrokeMgr = manager.cast()?;
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            // TSF-aware hosts route queued keys through the keystroke manager before
            // TranslateMessage (including preserved Ctrl+Space). RichEdit handles text storage.
            let eaten = if message.message == WM_KEYDOWN {
                keys.TestKeyDown(message.wParam, message.lParam)
                    .unwrap_or_default()
                    .as_bool()
                    && keys
                        .KeyDown(message.wParam, message.lParam)
                        .unwrap_or_default()
                        .as_bool()
            } else if message.message == WM_KEYUP {
                keys.TestKeyUp(message.wParam, message.lParam)
                    .unwrap_or_default()
                    .as_bool()
                    && keys
                        .KeyUp(message.wParam, message.lParam)
                        .unwrap_or_default()
                        .as_bool()
            } else {
                false
            };
            if eaten {
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        manager.Deactivate()?;
        CoUninitialize();
    }
    Ok(())
}
