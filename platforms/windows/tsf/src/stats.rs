//! Count-only typing statistics. The TSF callback only queues small numeric deltas;
//! a worker writes them outside the host's input/edit transaction.
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::System::Registry::*;
use windows_core::w;

const IDLE_GAP: Duration = Duration::from_secs(15);
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Language {
    Chinese,
    English,
}

#[derive(Default)]
pub(crate) struct ActivityClock {
    last: Option<(Instant, Language)>,
}

impl ActivityClock {
    fn tick(&mut self, language: Language, now: Instant) -> u32 {
        let elapsed = self.last.and_then(|(last, previous)| {
            (previous == language).then(|| now.saturating_duration_since(last))
        });
        self.last = Some((now, language));
        elapsed
            .filter(|duration| *duration <= IDLE_GAP)
            .map_or(0, |duration| {
                duration.as_millis().min(u32::MAX as u128) as u32
            })
    }

    pub(crate) fn reset(&mut self) {
        self.last = None;
    }
}

#[derive(Clone, Copy, Default)]
struct Delta {
    timestamp_ms: u64,
    chinese: u32,
    english: u32,
    chinese_active_ms: u32,
    english_active_ms: u32,
}

impl Delta {
    fn add(&mut self, other: Self) {
        self.chinese = self.chinese.saturating_add(other.chinese);
        self.english = self.english.saturating_add(other.english);
        self.chinese_active_ms = self
            .chinese_active_ms
            .saturating_add(other.chinese_active_ms);
        self.english_active_ms = self
            .english_active_ms
            .saturating_add(other.english_active_ms);
    }
}

static SENDER: OnceLock<Option<SyncSender<Delta>>> = OnceLock::new();

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u64::MAX as u128) as u64
        })
}

fn directory() -> Option<PathBuf> {
    let mut buffer = [0u16; 32768];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: UTF-16 buffer and byte length match. A provisioned absolute path is
    // needed because AppContainer LOCALAPPDATA points at a different profile.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\retype"),
            w!("StatisticsPath"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if result.is_ok() {
        let length = buffer.iter().position(|ch| *ch == 0)?;
        let path = PathBuf::from(String::from_utf16_lossy(&buffer[..length]));
        return path.is_absolute().then_some(path);
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("retype").join("statistics"))
}

fn sender() -> Option<&'static SyncSender<Delta>> {
    SENDER
        .get_or_init(|| {
            let root = directory()?;
            let (sender, receiver) = mpsc::sync_channel(1024);
            std::thread::Builder::new()
                .name("retype-statistics".into())
                .spawn(move || writer(receiver, root))
                .ok()?;
            Some(sender)
        })
        .as_ref()
}

fn enqueue(delta: Delta) {
    if cfg!(test) {
        // TSF integration tests exercise the real edit path without changing
        // the developer's personal statistics.
        return;
    }
    if let Some(sender) = sender() {
        // Never wait for disk or another process in an input callback.
        let _ = sender.try_send(delta);
    }
}

pub(crate) fn activity(clock: &Mutex<ActivityClock>, language: Language) {
    let elapsed = clock
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .tick(language, Instant::now());
    if elapsed == 0 {
        return;
    }
    let mut delta = Delta {
        timestamp_ms: now_ms(),
        ..Delta::default()
    };
    match language {
        Language::Chinese => delta.chinese_active_ms = elapsed,
        Language::English => delta.english_active_ms = elapsed,
    }
    enqueue(delta);
}

pub(crate) fn commit(text: &str) {
    let (chinese, english) = count(text);
    if chinese + english > 0 {
        enqueue(Delta {
            timestamp_ms: now_ms(),
            chinese,
            english,
            ..Delta::default()
        });
    }
}

fn count(text: &str) -> (u32, u32) {
    text.chars().fold((0u32, 0u32), |(chinese, english), ch| {
        if is_han(ch) {
            (chinese.saturating_add(1), english)
        } else if ch.is_ascii_alphabetic() {
            (chinese, english.saturating_add(1))
        } else {
            (chinese, english)
        }
    })
}

fn is_han(ch: char) -> bool {
    matches!(ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF |
        0x20000..=0x2A6DF | 0x2A700..=0x2B73F | 0x2B740..=0x2B81F |
        0x2B820..=0x2CEAF | 0x2CEB0..=0x2EBEF | 0x30000..=0x323AF)
}

fn writer(receiver: Receiver<Delta>, root: PathBuf) {
    if std::fs::create_dir_all(&root).is_err() {
        return;
    }
    let started = now_ms();
    let file = root.join(format!("{}-{started}.log", std::process::id()));
    let mut pending = BTreeMap::<u64, Delta>::new();
    let mut next_flush = Instant::now() + FLUSH_INTERVAL;
    loop {
        match receiver.recv_timeout(next_flush.saturating_duration_since(Instant::now())) {
            Ok(delta) => {
                pending
                    .entry(delta.timestamp_ms / 1000)
                    .and_modify(|total| total.add(delta))
                    .or_insert(delta);
                for _ in 0..4096 {
                    let Ok(delta) = receiver.try_recv() else {
                        break;
                    };
                    pending
                        .entry(delta.timestamp_ms / 1000)
                        .and_modify(|total| total.add(delta))
                        .or_insert(delta);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                flush(&file, &mut pending);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if Instant::now() >= next_flush {
            flush(&file, &mut pending);
            next_flush = Instant::now() + FLUSH_INTERVAL;
        }
    }
}

fn flush(path: &PathBuf, pending: &mut BTreeMap<u64, Delta>) {
    if pending.is_empty() {
        return;
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let mut lines = String::new();
    for (second, delta) in pending.iter() {
        use std::fmt::Write as _;
        let _ = writeln!(
            lines,
            "{second},{},{},{},{}",
            delta.chinese, delta.english, delta.chinese_active_ms, delta.english_active_ms
        );
    }
    let original_len = file.metadata().map_or(0, |metadata| metadata.len());
    if file.write_all(lines.as_bytes()).is_ok() {
        pending.clear();
    } else {
        // This process owns this log file. Roll back a partial batch before
        // retrying, so complete lines are never counted twice.
        let _ = file.set_len(original_len);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;

    #[test]
    fn counts_only_han_and_english_letters() {
        assert_eq!(count("你好，retype 123!𠀀"), (3, 6));
        assert_eq!(count("拼音 nihao。"), (2, 5));
    }

    #[test]
    fn activity_clock_excludes_idle_and_mode_switches() {
        let start = Instant::now();
        let mut clock = ActivityClock::default();
        assert_eq!(clock.tick(Language::Chinese, start), 0);
        assert_eq!(
            clock.tick(Language::Chinese, start + Duration::from_millis(300)),
            300
        );
        assert_eq!(
            clock.tick(Language::English, start + Duration::from_millis(500)),
            0
        );
        assert_eq!(
            clock.tick(Language::English, start + Duration::from_secs(20)),
            0
        );
    }

    #[test]
    fn writer_persists_only_numeric_deltas() {
        let root = std::env::temp_dir().join(format!(
            "retype-statistics-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let (sender, receiver) = mpsc::sync_channel(8);
        let worker = std::thread::spawn({
            let root = root.clone();
            move || writer(receiver, root)
        });
        sender
            .send(Delta {
                timestamp_ms: 1_000_000,
                chinese: 2,
                english: 3,
                chinese_active_ms: 400,
                english_active_ms: 500,
            })
            .expect("statistics receiver");
        drop(sender);
        worker.join().expect("statistics worker");
        let log = std::fs::read_dir(&root)
            .expect("statistics directory")
            .next()
            .expect("statistics file")
            .expect("statistics entry")
            .path();
        assert_eq!(
            std::fs::read_to_string(log).expect("statistics data"),
            "1000,2,3,400,500\n"
        );
        std::fs::remove_dir_all(root).expect("remove test statistics");
    }
}
