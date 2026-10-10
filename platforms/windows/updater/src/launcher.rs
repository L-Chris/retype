//! Stable GUI-subsystem task entry: starts the currently active updater without a console.
#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
use retype_updater_cli::installation;

#[cfg(windows)]
fn main() {
    if let Err(error) = installation::launch_background() {
        if let Some(root) = std::env::var_os("LOCALAPPDATA") {
            let directory = std::path::PathBuf::from(root).join("retype/updates");
            let _ = std::fs::create_dir_all(&directory);
            let _ = std::fs::write(directory.join("launcher-error.log"), format!("{error}\n"));
        }
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("The background update launcher is only supported on Windows.");
}
