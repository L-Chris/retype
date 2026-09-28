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
/// Open the installed settings app without loading Flutter into the TSF host.
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
    std::process::Command::new(path)
        .spawn()
        .map_err(|_| windows_core::Error::from(windows::Win32::Foundation::E_FAIL))?;
    Ok(())
}
