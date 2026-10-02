#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod ai;
#[cfg(windows)]
mod app;
#[cfg(windows)]
mod backend;
#[cfg(windows)]
mod cloud;
#[cfg(windows)]
mod instance;
#[cfg(windows)]
#[path = "../../../platforms/windows/common/settings_log.rs"]
mod settings_log;
#[cfg(windows)]
mod statistics;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    if std::env::args().any(|arg| arg == "--sync-dictionaries") {
        return if backend::sync_enabled_packs().is_ok() {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        };
    }
    settings_log::event("app", "process_start", 0, "entry");
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("file={} line={}", l.file(), l.line()))
            .unwrap_or_default();
        settings_log::event("app", "panic", 0, location);
        settings_log::flush();
        previous(info);
    }));
    if let Err(error) = app::run() {
        settings_log::event(
            "app",
            "startup_failed",
            0,
            "see preceding startup stage and legacy error report",
        );
        settings_log::flush();
        // Surface startup failures even when no console is attached.
        let path = std::env::temp_dir().join("retype-settings-egui-error.txt");
        let _ = std::fs::write(path, error.to_string());
        #[allow(unsafe_code)]
        unsafe {
            // SAFETY: Both strings remain alive for the synchronous message box.
            let message = backend::wide(&format!("无法打开设置：{error}"));
            windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                None,
                windows::core::PCWSTR(message.as_ptr()),
                windows::core::w!("retype 设置"),
                windows::Win32::UI::WindowsAndMessaging::MB_OK
                    | windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
            );
        }
        return std::process::ExitCode::FAILURE;
    }
    settings_log::event("app", "process_exit", 0, "normal");
    settings_log::flush();
    std::process::ExitCode::SUCCESS
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The settings application currently targets Windows.");
}
