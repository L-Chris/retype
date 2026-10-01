#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn serve() -> Result<(), Box<dyn std::error::Error>> {
    use retype_learning::{protocol, store::Store, transport};
    use std::path::PathBuf;
    use std::sync::Arc;

    let args: Vec<String> = std::env::args().collect();
    let custom_db = args
        .windows(2)
        .find(|pair| pair[0] == "--db")
        .map(|pair| PathBuf::from(&pair[1]));
    let sid = transport::user_sid()?;
    let name = if custom_db.is_some() {
        // Isolated integration tests never touch the installed learning database or pipe.
        args.windows(2)
            .find(|pair| pair[0] == "--pipe")
            .map(|pair| pair[1].clone())
            .ok_or("--db requires --pipe")?
    } else {
        transport::pipe_name(&sid)
    };
    if args.iter().any(|arg| arg == "--stop") {
        let request = protocol::Request {
            version: protocol::VERSION,
            client: "broker-stop".into(),
            known_revision: None,
            events: Vec::new(),
            stop: true,
        };
        transport::exchange(&name, &serde_json::to_vec(&request)?)?;
        return Ok(());
    }
    // Exclusivity is acquired before opening SQLite, including during an upgrade handoff.
    let listener = transport::Listener::new(&name, &sid)?;
    let db_path = if let Some(path) = custom_db.clone() {
        path
    } else {
        let root = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA missing")?);
        if !root.is_absolute() {
            return Err("LOCALAPPDATA must be absolute".into());
        }
        root.join("retype/learning/user.db")
    };
    if let Some(parent) = db_path.parent() {
        if custom_db.is_none() {
            transport::private_directory(parent, &sid)?;
        } else {
            std::fs::create_dir_all(parent)?;
        }
    }
    let directory = std::env::current_exe()?
        .parent()
        .ok_or("executable directory")?
        .to_path_buf();
    let system = retype_dict::AsyncDict::empty();
    let mut store = Store::open(
        &db_path,
        Arc::clone(&system) as Arc<dyn retype_pinyin::Lexicon>,
    )?;
    let dictionary = directory.join("retype-dict.bin");
    let _ = retype_dict::spawn_loader(
        dictionary,
        Arc::clone(&system),
        retype_dict::FallbackPolicy::SingleChar,
    );
    loop {
        if custom_db.is_none() {
            if let Some(active) = transport::read_machine_registry("ActiveDir") {
                let active = PathBuf::from(active);
                if active != directory {
                    let next = active.join("retype-learning-host.exe");
                    if next.is_file() {
                        drop(store);
                        drop(listener);
                        retype_learning::client::launch_host(Some(&next))?;
                        return Ok(());
                    }
                }
            }
        }
        if let Ok(bytes) = listener.receive(&sid) {
            if let Ok(request) = serde_json::from_slice::<protocol::Request>(&bytes) {
                if let Ok(response) = store.handle(&request) {
                    if let Ok(bytes) = serde_json::to_vec(&response) {
                        let _ = listener.respond(&bytes);
                    }
                    if request.stop {
                        return Ok(());
                    }
                }
            }
        }
        listener.disconnect();
    }
}

fn main() {
    #[cfg(windows)]
    if let Err(error) = serve() {
        // No modal error windows in an application's input path; the client keeps its cache.
        // --db diagnostics are isolated and may opt into a log path.
        if let Some(path) = std::env::args()
            .collect::<Vec<_>>()
            .windows(2)
            .find(|pair| pair[0] == "--error-log")
            .map(|pair| pair[1].clone())
        {
            let _ = std::fs::write(path, error.to_string());
        }
        std::process::exit(1);
    }
}
