#[cfg(windows)]
pub mod client;
pub mod config;
pub mod protocol;
#[cfg(windows)]
#[path = "credentials.rs"]
pub mod secrets;
#[cfg(feature = "service")]
pub mod service;
