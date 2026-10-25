//! Windows persistence adapter for the shared AI configuration.
pub use retype_ai_core::config::*;
use std::{io, path::PathBuf};
pub fn load() -> io::Result<Config> {
    Config::load_at(&path()?)
}
pub fn save(config: &Config) -> io::Result<()> {
    let path = path()?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("配置目录无效"))?;
    #[cfg(windows)]
    retype_learning::transport::private_directory(
        parent,
        &retype_learning::transport::user_sid()?,
    )?;
    #[cfg(not(windows))]
    std::fs::create_dir_all(parent)?;
    let temp = path.with_extension(format!("{}.tmp", new_id()));
    let result = (|| {
        std::fs::write(
            &temp,
            serde_json::to_vec_pretty(config).map_err(io::Error::other)?,
        )?;
        #[cfg(windows)]
        crate::secrets::replace_file(&temp, &path)?;
        #[cfg(not(windows))]
        std::fs::rename(&temp, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
fn path() -> io::Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or_else(|| io::Error::other("LOCALAPPDATA missing"))?,
    )
    .join("retype/ai/settings.json"))
}
