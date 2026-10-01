//! The TSF client keeps only a memory cache; the broker alone opens SQLite.
#[cfg(windows)]
pub mod client;
pub mod protocol;
#[cfg(feature = "broker")]
pub mod store;
#[cfg(windows)]
pub mod transport;
