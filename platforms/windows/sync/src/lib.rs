pub mod clipboard_protocol;
pub mod config;
#[cfg(windows)]
pub mod lan;
pub mod model;
pub mod pairing;
#[cfg(windows)]
pub mod runtime;
pub mod statistics;
pub mod webdav;
pub type Result<T> = std::result::Result<T, String>;

pub fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
