pub use retype_ai_core::voice::*;
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn microphones() -> Vec<(u32, String)> {
    use windows_sys::Win32::Media::Audio::*;
    let mut devices = vec![(u32::MAX, "系统默认输入设备".into())];
    // SAFETY: read-only WinMM enumeration; fixed-size caps buffer matches its byte length.
    unsafe {
        for id in 0..waveInGetNumDevs().min(128) {
            let mut caps: WAVEINCAPSW = std::mem::zeroed();
            if waveInGetDevCapsW(
                id as usize,
                &mut caps,
                std::mem::size_of::<WAVEINCAPSW>() as u32,
            ) == 0
            {
                let name = caps.szPname;
                let len = name.iter().position(|ch| *ch == 0).unwrap_or(name.len());
                devices.push((id, String::from_utf16_lossy(&name[..len])));
            }
        }
    }
    devices
}
