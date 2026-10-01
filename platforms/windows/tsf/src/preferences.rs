//! Small per-user preferences shared by the 32/64-bit TIPs.
use retype_types::PinyinScheme;
use windows::Win32::System::Registry::*;
use windows_core::{w, Result};

pub fn scheme() -> PinyinScheme {
    let mut value = 0u32;
    let mut size = 4;
    // SAFETY: DWORD output buffer has the advertised size. Missing/invalid values use full pinyin.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\retype"),
            w!("PinyinScheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if result.is_ok() && value == 1 {
        PinyinScheme::Flypy
    } else {
        PinyinScheme::Full
    }
}

fn read_dword(name: windows_core::PCWSTR) -> u32 {
    let mut value = 0u32;
    let mut size = 4;
    // SAFETY: The output buffer is a DWORD and failures leave the default zero.
    unsafe {
        let _ = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\retype"),
            name,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        );
    }
    value
}

pub fn pack_generation() -> u32 {
    read_dword(w!("DictionaryGeneration"))
}

pub fn enabled_packs() -> u32 {
    read_dword(w!("EnabledDictionaryPacks"))
}

pub fn pack_root() -> Option<std::path::PathBuf> {
    let mut buffer = [0u16; 32768];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: Fixed UTF-16 output buffer, read-only HKCU value.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\retype"),
            w!("DictionaryRoot"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if result.is_err() {
        return None;
    }
    let length = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    if length == 0 {
        None
    } else {
        Some(std::path::PathBuf::from(String::from_utf16_lossy(
            &buffer[..length],
        )))
    }
}
/// Open the installed settings app without loading a UI runtime into the TSF host.
pub fn open_settings() -> Result<()> {
    let mut buffer = [0u16; 32768];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: Fixed UTF-16 buffer; the x86 TIP must read the x64 installation view.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("Software\\retype"),
            w!("ActiveDir"),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
        .ok()?;
    }
    let length = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    let path = std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..length]))
        .join("settings")
        .join("retype.exe");
    let expected: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
    // Reuse a live settings window only when it belongs to the active install.
    // This avoids loading a second settings process merely to raise the first.
    unsafe {
        use windows::Win32::System::Threading::*;
        use windows::Win32::UI::WindowsAndMessaging::*;
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
            if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                let mut executable = [0u16; 32768];
                let mut len = executable.len() as u32;
                let found = QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_WIN32,
                    windows_core::PWSTR(executable.as_mut_ptr()),
                    &mut len,
                )
                .is_ok();
                let _ = windows::Win32::Foundation::CloseHandle(process);
                let matches = found
                    && windows::Win32::Globalization::CompareStringOrdinal(
                        &expected,
                        &executable[..len as usize],
                        true,
                    ) == windows::Win32::Globalization::CSTR_EQUAL;
                if matches {
                    let _ = AllowSetForegroundWindow(pid);
                    // Shared with the settings app; reactivate its retained window.
                    let _ = PostMessageW(
                        Some(window),
                        WM_APP + 29,
                        windows::Win32::Foundation::WPARAM(0),
                        windows::Win32::Foundation::LPARAM(0),
                    );
                    return Ok(());
                }
            }
        }
    }
    std::process::Command::new(path)
        .spawn()
        .map_err(|_| windows_core::Error::from(windows::Win32::Foundation::E_FAIL))?;
    Ok(())
}
