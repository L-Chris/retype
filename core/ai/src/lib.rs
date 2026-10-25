//! Shared AI configuration, translation and voice sessions; callers supply credentials and audio.
pub mod config;
pub mod protocol;
#[cfg(feature = "service")]
pub mod service;
pub mod voice;
#[cfg(feature = "service")]
mod voice_live;
#[cfg(feature = "service")]
pub mod voice_service;
