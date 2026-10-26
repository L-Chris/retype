//! Desktop clipboard, process and configuration adapters for shared synchronization.
pub use retype_sync_core::{
    clipboard_image, clipboard_protocol, hash, model, pairing, schema, statistics, webdav, Result,
};
pub mod config;
#[cfg(any(windows, target_os = "macos"))]
pub mod lan;
#[cfg(windows)]
pub mod runtime;
