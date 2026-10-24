// The count-only Windows writer is deliberately portable: exercise its state
// machine and file format on development hosts without touching personal logs.
#![cfg(not(windows))]
#![allow(dead_code)]
#[path = "../../../platforms/windows/tsf/src/stats.rs"]
mod stats;
