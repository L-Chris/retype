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
