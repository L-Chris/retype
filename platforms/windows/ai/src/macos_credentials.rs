//! Keychain access is supplied by the native AppKit host; no external commands.
#![allow(unsafe_code)]
use std::{
    ffi::{c_char, CString},
    io,
    sync::OnceLock,
};
pub type Read = unsafe extern "C" fn(*const c_char, *mut u8, usize) -> isize;
pub type Save = unsafe extern "C" fn(*const c_char, *const c_char) -> bool;
static ACCESS: OnceLock<(Read, Save)> = OnceLock::new();
pub fn register(read: Read, save: Save) {
    let _ = ACCESS.set((read, save));
}
fn read(account: &str) -> io::Result<String> {
    let (read, _) = ACCESS
        .get()
        .ok_or_else(|| io::Error::other("钥匙串尚未就绪"))?;
    let name = CString::new(account).map_err(io::Error::other)?;
    let mut output = vec![0u8; 65536];
    let count = unsafe { read(name.as_ptr(), output.as_mut_ptr(), output.len()) };
    if count < 0 || count as usize > output.len() {
        return Err(io::Error::other("无法读取钥匙串"));
    }
    output.truncate(count as usize);
    String::from_utf8(output).map_err(io::Error::other)
}
fn save(account: &str, value: &str) -> io::Result<()> {
    let (_, save) = ACCESS
        .get()
        .ok_or_else(|| io::Error::other("钥匙串尚未就绪"))?;
    let name = CString::new(account).map_err(io::Error::other)?;
    let value = CString::new(value).map_err(io::Error::other)?;
    if unsafe { save(name.as_ptr(), value.as_ptr()) } {
        Ok(())
    } else {
        Err(io::Error::other("无法写入钥匙串"))
    }
}
pub fn key(id: &str) -> io::Result<String> {
    read(&format!("ai/{id}"))
}
pub fn save_key(id: &str, value: &str) -> io::Result<()> {
    save(&format!("ai/{id}"), value)
}
pub fn cloud_password(account: &str) -> io::Result<String> {
    read(&format!("sync/{account}"))
}
pub fn save_cloud_password(account: &str, value: &str) -> io::Result<()> {
    save(&format!("sync/{account}"), value)
}
