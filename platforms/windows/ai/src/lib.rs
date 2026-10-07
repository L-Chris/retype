#[cfg(windows)]
pub mod client;
pub mod config;
#[cfg(all(windows, feature = "service"))]
pub mod microphone;
pub mod protocol;
#[cfg(windows)]
#[path = "credentials.rs"]
pub mod secrets;
#[cfg(feature = "service")]
pub mod service;
pub mod voice;
#[cfg(feature = "service")]
mod voice_live;
#[cfg(feature = "service")]
pub mod voice_service;
