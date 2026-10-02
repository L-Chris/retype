#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() {
    #[cfg(windows)]
    if std::env::args().any(|arg| arg == "--stop") {
        if let Ok(root) = retype_sync::config::root() {
            let _ = std::fs::write(root.join("stop"), []);
        }
        return;
    }
    #[cfg(windows)]
    if let Err(error) = retype_sync::runtime::serve() {
        if let Ok(root) = retype_sync::config::root() {
            if let Ok(config) = retype_sync::config::Config::load_at(&root) {
                let state = retype_sync::runtime::Status {
                    message: error,
                    ..Default::default()
                };
                let _ = retype_sync::config::write_json(
                    &root.join(config.account()).join("status.json"),
                    &state,
                );
            }
        }
    }
}
