//! Shared persistence and interchange format. No platform APIs.
pub mod protocol;
#[cfg(feature = "sqlite")]
pub mod store;
