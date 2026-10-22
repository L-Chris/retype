//! User-profile-published dictionary paths: AppContainer LOCALAPPDATA is redirected.
use std::path::{Path, PathBuf};
use windows::{
    core::{w, PCWSTR},
    Win32::System::Registry::*,
};
#[allow(unsafe_code)]
fn text(hive: HKEY, name: &str) -> Option<PathBuf> {
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let mut buffer = [0u16; 4096];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: fixed registry key, valid terminated name and bounded UTF-16 buffer.
    let result = unsafe {
        RegGetValueW(
            hive,
            w!("Software\\retype"),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if result.is_err() || bytes < 2 || bytes as usize > buffer.len() * 2 {
        return None;
    }
    let path = PathBuf::from(String::from_utf16_lossy(&buffer[..bytes as usize / 2 - 1]));
    path.is_absolute().then_some(path)
}
pub fn root() -> Option<PathBuf> {
    std::env::var_os("RETYPE_BASE_RESOURCES")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| text(HKEY_CURRENT_USER, "BaseDictionaryRoot"))
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|p| p.join("retype/dictionaries"))
        })
}
pub fn legacy(hint: Option<&Path>) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(path) = hint {
        directories.push(path.to_path_buf());
        if path.file_name().is_some_and(|n| n == "x86") {
            if let Some(p) = path.parent() {
                directories.push(p.to_path_buf());
            }
        }
    }
    if let Some(active) = text(HKEY_LOCAL_MACHINE, "ActiveDir") {
        directories.push(active.clone());
        if let Some(parent) = active
            .parent()
            .filter(|p| p.file_name().is_some_and(|n| n == "versions"))
        {
            if let Ok(entries) = std::fs::read_dir(parent) {
                directories.extend(entries.filter_map(Result::ok).take(128).map(|e| e.path()));
            }
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        directories.push(PathBuf::from(local).join("retype"));
    }
    let mut paths = Vec::new();
    for directory in directories {
        let file = directory.join("retype-dict.bin");
        if !paths.contains(&file) {
            paths.push(file);
        }
    }
    paths
}

/// Worker-only lookup. Compatible resources from older installations stay usable.
#[allow(dead_code)] // Shared with the settings process, which migrates before lookup.
pub fn available(root: &Path, hint: Option<&Path>) -> Option<PathBuf> {
    retype_resources::status(root)
        .ok()
        .and_then(|s| s.base)
        .or_else(|| legacy(hint).into_iter().find(|p| p.is_file()))
}
