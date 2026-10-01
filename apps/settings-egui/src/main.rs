#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod backend;
#[cfg(windows)]
mod instance;
#[cfg(windows)]
mod statistics;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    if let Err(error) = app::run() {
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
    std::process::ExitCode::SUCCESS
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The settings application currently targets Windows.");
}
