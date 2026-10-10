//! Machine-owned active installation metadata, read in the 64-bit registry view.
use std::{os::windows::process::CommandExt, path::PathBuf};
use windows::{
    core::{w, PCWSTR},
    Win32::{Foundation::*, System::Registry::*},
};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[allow(unsafe_code)]
pub fn installed(name: &str) -> Result<String> {
    let name: Vec<_> = name.encode_utf16().chain([0]).collect();
    let mut data = vec![0u16; 32768];
    let mut size = (data.len() * 2) as u32;
    // SAFETY: Owned UTF-16 buffer and terminated names; no registry handle is retained.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("Software\\retype"),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()?;
    }
    let end = data.iter().position(|v| *v == 0).unwrap_or(data.len());
    Ok(String::from_utf16(&data[..end])?)
}

pub fn active_directory() -> Result<PathBuf> {
    let directory = PathBuf::from(installed("ActiveDir")?);
    if !directory.is_absolute() || !directory.join("retype-updater.exe").is_file() {
        return Err("retype installation is incomplete".into());
    }
    Ok(directory)
}

#[allow(unsafe_code)]
pub fn auto_check() -> Result<Option<bool>> {
    let mut value = 0u32;
    let mut size = 4;
    // SAFETY: The four-byte output matches REG_DWORD.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\retype"),
            w!("AutoCheck"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if result == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    result.ok()?;
    Ok(Some(value != 0))
}

pub fn launch_background() -> Result<()> {
    std::process::Command::new(active_directory()?.join("retype-updater.exe"))
        .args(["update", "--background"])
        .creation_flags(0x08000000)
        .spawn()?;
    Ok(())
}
