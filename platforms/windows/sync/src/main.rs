#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() {
    #[cfg(windows)]
    {
        if std::env::args().any(|arg| arg == "--stop") {
            if let Ok(root) = retype_sync::config::root() {
                let _ = std::fs::write(root.join("stop"), []);
                let _ = std::fs::write(root.join("lan/stop"), []);
            }
            return;
        }
        std::thread::spawn(|| loop {
            if let Err(error) = retype_sync::runtime::serve() {
                if let Ok(root) = retype_sync::config::root() {
                    if let Ok(config) = retype_sync::config::Config::load_at(&root) {
                        let status = retype_sync::runtime::Status {
                            message: error,
                            ..Default::default()
                        };
                        let _ = retype_sync::config::write_json(
                            &root.join(config.account()).join("status.json"),
                            &status,
                        );
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        });
        if let Err(error) = retype_sync::lan::serve() {
            if let Ok(root) = retype_sync::lan::root() {
                let _ = retype_sync::config::write_json(
                    &root.join("status.json"),
                    &retype_sync::lan::Status {
                        message: error,
                        ..Default::default()
                    },
                );
            }
        }
    }
}
