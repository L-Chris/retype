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
pub fn save_scheme(scheme: PinyinScheme) -> Result<()> {
    let value = u32::from(scheme == PinyinScheme::Flypy);
    // SAFETY: RegSetKeyValue creates the subkey as needed and copies this DWORD synchronously.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            w!("Software\\retype"),
            w!("PinyinScheme"),
            REG_DWORD.0,
            Some((&value as *const u32).cast()),
            4,
        )
        .ok()
    }
}

/// Launch outside the host process; always follow the current machine installation.
pub fn open_updates() -> Result<()> {
    use std::os::windows::process::CommandExt;
    let mut buffer = [0u16; 32768];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: Fixed UTF-16 output buffer; force 64-bit view even in an x86 host.
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
    let path =
        std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..length])).join("update-ui.ps1");
    let root = std::env::var_os("SystemRoot")
        .ok_or_else(|| windows_core::Error::from(windows::Win32::Foundation::E_FAIL))?;
    std::process::Command::new(
        std::path::PathBuf::from(root).join(if cfg!(target_arch = "x86") {
            "Sysnative\\WindowsPowerShell\\v1.0\\powershell.exe"
        } else {
            "System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        }),
    )
    .args([
        "-NoProfile",
        "-STA",
        "-WindowStyle",
        "Hidden",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ])
    .arg(path)
    .creation_flags(0x08000000) // CREATE_NO_WINDOW: only the update form is visible.
    .spawn()
    .map_err(|_| windows_core::Error::from(windows::Win32::Foundation::E_FAIL))?;
    Ok(())
}
