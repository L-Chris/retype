use crate::Result;
pub use retype_sync_core::config::*;
use std::path::PathBuf;
pub fn root() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    return Ok(
        PathBuf::from(std::env::var_os("HOME").ok_or("无法找到用户目录")?)
            .join("Library/Application Support/retype/sync"),
    );
    #[cfg(not(target_os = "macos"))]
    Ok(
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("无法找到本地数据目录")?)
            .join("retype/sync"),
    )
}
