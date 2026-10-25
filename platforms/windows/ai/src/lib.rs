#[cfg(windows)]
pub mod client;
pub mod config;
#[cfg(all(windows, feature = "service"))]
pub mod microphone;
pub use retype_ai_core::protocol;
#[cfg(windows)]
#[path = "credentials.rs"]
pub mod secrets;
#[cfg(target_os = "macos")]
#[path = "macos_credentials.rs"]
pub mod secrets;
#[cfg(feature = "service")]
pub mod service;
pub mod voice;
#[cfg(feature = "service")]
pub use retype_ai_core::voice_service;
