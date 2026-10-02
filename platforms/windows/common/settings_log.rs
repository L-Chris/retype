//! Settings and explicit translation lifecycle metadata only.
//! Never pass input text, credentials or raw CLI arguments.
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc::{self, SyncSender},
        OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const MAX_BYTES: u64 = 2 * 1024 * 1024;
enum Message {
    Record(String),
    Flush(mpsc::Sender<()>),
}
struct Logger {
    sender: SyncSender<Message>,
}
static LOGGER: OnceLock<Option<Logger>> = OnceLock::new();
static SERIAL: AtomicU32 = AtomicU32::new(0);
pub fn request_id() -> u32 {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u32;
    seed.wrapping_add(SERIAL.fetch_add(1, Ordering::Relaxed))
        .wrapping_add(std::process::id())
        .max(1)
}
pub fn event(
    role: &'static str,
    stage: &'static str,
    request: u32,
    detail: impl std::fmt::Display,
) {
    let logger = LOGGER.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel(128);
        if std::thread::Builder::new()
            .name("retype-settings-log".into())
            .spawn(move || {
                let Some(root) = log_root() else {
                    return;
                };
                let fallback = std::env::temp_dir().join("retype-settings-logs");
                let primary = fs::create_dir_all(&root).is_ok();
                let mut root = if primary { root } else { fallback.clone() };
                let _ = fs::create_dir_all(&root);
                prune_old(&root);
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Record(line) => {
                            if append(&root, role, &line).is_err() && root != fallback {
                                root = fallback.clone();
                                let _ = fs::create_dir_all(&root);
                                let _ = append(&root, role, &line);
                            }
                        }
                        Message::Flush(ack) => {
                            let _ = ack.send(());
                        }
                    }
                }
            })
            .is_err()
        {
            return None;
        }
        Some(Logger { sender })
    });
    let Some(logger) = logger else {
        return;
    };
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let record = serde_json::json!({
        "time_ms":time, "pid":std::process::id(), "role":role,
        "version":env!("CARGO_PKG_VERSION"), "arch":std::env::consts::ARCH,
        "request":request, "event":stage, "detail":detail.to_string(),
    });
    if let Ok(line) = serde_json::to_string(&record) {
        let _ = logger.sender.try_send(Message::Record(line));
    }
}
/// App exit or monitor worker only, never a TSF callback. A bounded barrier drains queued records.
pub fn flush() {
    if let Some(Some(logger)) = LOGGER.get() {
        let (sender, receiver) = mpsc::channel();
        if logger.sender.try_send(Message::Flush(sender)).is_ok() {
            let _ = receiver.recv_timeout(Duration::from_millis(300));
        }
    }
}
fn append(root: &Path, role: &str, line: &str) -> io::Result<()> {
    let path = root.join(format!("settings-{role}-{}.jsonl", std::process::id()));
    if fs::metadata(&path).is_ok_and(|meta| meta.len() + line.len() as u64 + 1 > MAX_BYTES) {
        let previous = path.with_extension("previous.jsonl");
        if previous.exists() {
            fs::remove_file(&previous)?;
        }
        fs::rename(&path, &previous)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    file.flush()
}
fn prune_old(root: &Path) {
    let Ok(files) = fs::read_dir(root) else {
        return;
    };
    for entry in files.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let own_log = name
            .strip_prefix("settings-app-")
            .or_else(|| name.strip_prefix("settings-launcher-"))
            .and_then(|tail| {
                tail.strip_suffix(".previous.jsonl")
                    .or_else(|| tail.strip_suffix(".jsonl"))
            })
            .and_then(|pid| pid.parse::<u32>().ok())
            .is_some();
        if own_log
            && entry.file_type().is_ok_and(|t| t.is_file())
            && entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(7 * 24 * 60 * 60))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}
fn log_root() -> Option<PathBuf> {
    if cfg!(test) {
        return Some(
            std::env::temp_dir().join(format!("retype-settings-log-tests-{}", std::process::id())),
        );
    }
    #[cfg(windows)]
    {
        use windows::{core::w, Win32::System::Registry::*};
        let mut buffer = [0u16; 32768];
        let mut len = (buffer.len() * 2) as u32;
        // SAFETY: bounded aligned UTF-16 output buffer, read-only per-user preference.
        #[allow(unsafe_code)]
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Software\\retype"),
                w!("SettingsLogPath"),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut len),
            )
        };
        if result.is_ok() {
            let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
            let root = PathBuf::from(String::from_utf16_lossy(&buffer[..end]));
            if root.is_absolute() {
                return Some(root);
            }
        }
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.join("retype/logs"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotation_retains_latest_event_as_valid_utf8_json() -> io::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "retype-settings-log-test-{}-{}",
            std::process::id(),
            request_id()
        ));
        fs::create_dir(&root)?;
        let path = root.join(format!("settings-test-{}.jsonl", std::process::id()));
        fs::write(&path, vec![b'x'; MAX_BYTES as usize])?;
        let event = serde_json::json!({"event":"visible","detail":"窗口就绪"}).to_string();
        append(&root, "test", &event)?;
        let read = fs::read_to_string(&path)?;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&read)
                .ok()
                .map(|v| v["event"].clone()),
            Some(serde_json::json!("visible"))
        );
        let previous = path.with_extension("previous.jsonl");
        assert_eq!(fs::metadata(&previous)?.len(), MAX_BYTES);
        fs::remove_file(path)?;
        fs::remove_file(previous)?;
        fs::remove_dir(root)?;
        Ok(())
    }
}
