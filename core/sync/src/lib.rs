//! Shared synchronization schema, merge policy, clipboard protocol and pairing.
pub mod clipboard_image;
pub mod clipboard_protocol;
pub mod config;
pub mod lan_address;
pub mod model;
pub mod pairing;
pub mod schema;
pub mod statistics;
pub mod webdav;
pub type Result<T> = std::result::Result<T, String>;
pub fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
