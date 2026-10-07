//! Windows credential and preference access; no credential values are embedded in this file.
#![allow(unsafe_code)]
use retype_learning::transport::wide;
use std::{
    io,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*, Security::Credentials::*, Storage::FileSystem::*, System::Registry::*,
};
pub fn key(id: &str) -> io::Result<String> {
    read_credential(&format!("retype/ai/{id}"))
}
pub fn cloud_password(account: &str) -> io::Result<String> {
    read_credential(&format!("retype/sync/{account}"))
}
fn read_credential(target: &str) -> io::Result<String> {
    let name = wide(target);
    let mut credential = null_mut();
    // SAFETY: owned credential copied before CredFree.
    unsafe {
        if CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) == 0 {
            let e = io::Error::last_os_error();
            return if e.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
                Ok(String::new())
            } else {
                Err(e)
            };
        }
        let value = if (*credential).CredentialBlobSize == 0 {
            Ok(String::new())
        } else {
            String::from_utf8(
                std::slice::from_raw_parts(
                    (*credential).CredentialBlob,
                    (*credential).CredentialBlobSize as usize,
                )
                .to_vec(),
            )
            .map_err(io::Error::other)
        };
        CredFree(credential.cast());
        value
    }
}
pub fn save_key(id: &str, value: &str) -> io::Result<()> {
    write_credential(&format!("retype/ai/{id}"), value)
}
pub fn save_cloud_password(account: &str, value: &str) -> io::Result<()> {
    write_credential(&format!("retype/sync/{account}"), value)
}
fn write_credential(target: &str, value: &str) -> io::Result<()> {
    let name = wide(target);
    let user = wide("retype");
    // SAFETY: synchronous APIs copy the live input buffers.
    unsafe {
        if value.is_empty() {
            if CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) == 0
                && GetLastError() != ERROR_NOT_FOUND
            {
                return Err(io::Error::last_os_error());
            }
        } else {
            let c = CREDENTIALW {
                Type: CRED_TYPE_GENERIC,
                TargetName: name.as_ptr().cast_mut(),
                CredentialBlobSize: value.len() as u32,
                CredentialBlob: value.as_ptr().cast_mut(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                UserName: user.as_ptr().cast_mut(),
                ..std::mem::zeroed()
            };
            if CredWriteW(&c, 0) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}
pub fn replace_file(from: &std::path::Path, to: &std::path::Path) -> io::Result<()> {
    // SAFETY: nul-terminated paths; atomic same-volume replacement.
    if unsafe {
        MoveFileExW(
            wide(&from.to_string_lossy()).as_ptr(),
            wide(&to.to_string_lossy()).as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
pub fn shortcuts() -> crate::config::Shortcuts {
    let mut bytes = vec![0u16; 2048];
    let mut len = (bytes.len() * 2) as u32;
    // SAFETY: aligned, bounded UTF-16 registry output.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(r"Software\retype").as_ptr(),
            wide("Shortcuts").as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            bytes.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if status != ERROR_SUCCESS || len as usize > bytes.len() * 2 {
        return Default::default();
    }
    let end = bytes.iter().position(|c| *c == 0).unwrap_or(bytes.len());
    let mut value: crate::config::Shortcuts =
        serde_json::from_str(&String::from_utf16_lossy(&bytes[..end])).unwrap_or_default();
    // Old configurations may already use the new default chord for translation.
    let legacy =
        serde_json::from_str::<serde_json::Value>(&String::from_utf16_lossy(&bytes[..end]))
            .is_ok_and(|v| v.get("voice").is_none());
    if legacy && (value.voice == value.translate || value.voice == value.mode) {
        value.voice = crate::config::Shortcut::DISABLED;
    }
    if value.validate().is_ok() {
        value
    } else {
        Default::default()
    }
}
pub fn save_shortcuts(value: crate::config::Shortcuts) -> io::Result<()> {
    value.validate().map_err(io::Error::other)?;
    let mut key = null_mut();
    let text = wide(&serde_json::to_string(&value).map_err(io::Error::other)?);
    // SAFETY: owned registry handle, valid UTF-16 buffers; no secret data.
    unsafe {
        let s = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(r"Software\retype").as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null(),
            &mut key,
            null_mut(),
        );
        if s != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(s as i32));
        }
        let s = RegSetValueExW(
            key,
            wide("Shortcuts").as_ptr(),
            0,
            REG_SZ,
            text.as_ptr().cast(),
            (text.len() * 2) as u32,
        );
        RegCloseKey(key);
        if s != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(s as i32));
        }
    }
    Ok(())
}
