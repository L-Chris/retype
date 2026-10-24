use chrono::{Datelike, Local, NaiveDate, TimeZone, Timelike};
use retype_sync::statistics::{Bucket, Counts};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Default)]
pub struct Activity {
    last: Option<Instant>,
    elapsed: u64,
    words: retype_types::statistics::WordCounter,
    pub input: retype_types::statistics::InputTracker,
}
impl Activity {
    pub fn tick(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last {
            let d = now.duration_since(last);
            if d <= Duration::from_secs(15) {
                self.elapsed += d.as_millis() as u64;
            }
        }
        self.last = Some(now);
    }
    pub fn backspace(&mut self) -> Counts {
        self.words.backspace();
        Counts {
            input: self.input.backspace(),
            ..Default::default()
        }
    }
    pub fn commit(&mut self, text: &str) -> Counts {
        let mut counts = self.commit_counts(text);
        counts.input = self.input.commit(text);
        counts
    }
    pub fn commit_counts(&mut self, text: &str) -> Counts {
        let chinese = text.chars().filter(|c| matches!(*c as u32,0x3400..=0x4dbf|0x4e00..=0x9fff|0xf900..=0xfaff|0x20000..=0x323af)).count() as u64;
        let english = text.chars().filter(|c| c.is_ascii_alphanumeric()).count() as u64;
        let english_words = self.words.feed(text) as u64;
        let elapsed = std::mem::take(&mut self.elapsed);
        let chinese_ms = elapsed * chinese / (chinese + english).max(1);
        let english_ms = if english > 0 { elapsed - chinese_ms } else { 0 };
        Counts {
            chinese,
            english,
            chinese_ms,
            english_ms,
            english_words,
            english_word_ms: english_ms,
            ..Default::default()
        }
    }
    pub fn boundary(&mut self) -> Counts {
        self.last = None;
        self.elapsed = 0;
        Counts {
            english_words: self.words.finish() as u64,
            input: self.input.boundary(true),
            ..Default::default()
        }
    }
}
// Keep numeric deltas inline in the bounded channel; avoid allocating in input callbacks.
#[allow(clippy::large_enum_variant)]
enum Message {
    Delta(Counts, bool),
    Flush(mpsc::Sender<()>),
}
pub struct Writer(mpsc::SyncSender<Message>);
impl Writer {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::sync_channel::<Message>(1024);
        std::thread::Builder::new()
            .name("retype-mac-statistics".into())
            .spawn(move || {
                let Ok(mut file) =
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(root.join(retype_sync::statistics::versioned_stream(
                            "macos.log",
                            env!("CARGO_PKG_VERSION"),
                        )))
                else {
                    return;
                };
                let Ok(mut voice_file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(root.join(retype_sync::statistics::versioned_stream(
                        "macos-voice.log",
                        env!("CARGO_PKG_VERSION"),
                    )))
                else {
                    return;
                };
                for job in rx {
                    let (c, voice) = match job {
                        Message::Delta(c, v) => (c, v),
                        Message::Flush(done) => {
                            let _ = file.sync_all();
                            let _ = voice_file.sync_all();
                            let _ = done.send(());
                            continue;
                        }
                    };
                    let seconds = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let target = if voice { &mut voice_file } else { &mut file };
                    let record = retype_sync::statistics::LogRecord {
                        second: seconds,
                        version: env!("CARGO_PKG_VERSION").into(),
                        counts: c,
                    };
                    if let Ok(json) = serde_json::to_string(&record) {
                        let _ = writeln!(target, "{json}");
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self(tx))
    }
    pub fn record(&self, c: Counts) {
        if c != Counts::default() {
            let _ = self.0.try_send(Message::Delta(c, false));
        }
    }
    pub fn voice(&self, text: &str) {
        let (chinese, english) = retype_ai::voice::counts(text);
        let _ = self.0.try_send(Message::Delta(
            Counts {
                chinese: chinese.into(),
                english_words: english.into(),
                ..Default::default()
            },
            true,
        ));
    }
    pub fn flush(&self) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        self.0
            .send(Message::Flush(tx))
            .map_err(|_| "统计后台已退出")?;
        rx.recv_timeout(Duration::from_secs(2))
            .map_err(|_| "保存统计数据超时".into())
    }
}
pub fn export(root: &Path, device: &str) -> Result<Vec<Bucket>, String> {
    let mut rows = retype_sync::statistics::capture(&root.join("learning.statistics"), device)?;
    let imported = root.join("statistics-imported.json");
    if imported.exists() {
        let history: Vec<Bucket> =
            serde_json::from_slice(&std::fs::read(imported).map_err(|e| e.to_string())?)
                .map_err(|_| "统计数据损坏")?;
        retype_sync::statistics::merge(&mut rows, &history)?;
    }
    Ok(rows)
}
fn date(year: i32, month: u32, day: u32) -> Result<NaiveDate, String> {
    NaiveDate::from_ymd_opt(year, month, day).ok_or("统计日期无效".into())
}
pub fn summary(rows: &[Bucket], period: u32, version: Option<&str>) -> Result<Value, String> {
    let versions: std::collections::BTreeSet<_> = rows
        .iter()
        .filter(|r| {
            retype_sync::statistics::is_stream(&r.stream, "macos.log")
                || retype_sync::statistics::is_stream(&r.stream, "macos-voice.log")
        })
        .map(|r| r.version.clone())
        .collect();
    let selected: Vec<_> = rows
        .iter()
        .filter(|r| version.is_none_or(|v| v == r.version))
        .cloned()
        .collect();
    let rows = &selected;
    let now = Local::now();
    let today = now.date_naive();
    let start = match period {
        0 => today,
        1 => today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64),
        2 => date(today.year(), today.month(), 1)?,
        _ => date(today.year(), 1, 1)?,
    };
    let previous = match period {
        0 => start - chrono::Duration::days(1),
        1 => start - chrono::Duration::days(7),
        2 => {
            if start.month() == 1 {
                date(start.year() - 1, 12, 1)?
            } else {
                date(start.year(), start.month() - 1, 1)?
            }
        }
        _ => date(start.year() - 1, 1, 1)?,
    };
    let mut current = Counts::default();
    let mut old = Counts::default();
    let mut recent = Counts::default();
    let mut voice = Counts::default();
    for row in rows
        .iter()
        .filter(|r| retype_sync::statistics::is_stream(&r.stream, "macos-voice.log"))
    {
        if let Some(when) = Local.timestamp_opt((row.minute * 60) as i64, 0).single() {
            if when.date_naive() >= start && when <= now {
                voice.add(&row.counts);
            }
        }
    }
    let mut bars = std::collections::BTreeMap::<u32, Counts>::new();
    let slots = match period {
        0 => 24,
        1 => 7,
        2 => (if start.month() == 12 {
            date(start.year() + 1, 1, 1)?
        } else {
            date(start.year(), start.month() + 1, 1)?
        } - start)
            .num_days() as u32,
        _ => 12,
    };
    for i in 0..slots {
        bars.insert(i, Counts::default());
    }
    for row in rows
        .iter()
        .filter(|r| retype_sync::statistics::is_stream(&r.stream, "macos.log"))
    {
        let Some(when) = Local.timestamp_opt((row.minute * 60) as i64, 0).single() else {
            continue;
        };
        if when > now {
            continue;
        }
        if when.timestamp() >= now.timestamp() - 300 {
            recent.add(&row.counts);
        }
        if when.date_naive() >= start {
            current.add(&row.counts);
            let key = match period {
                0 => when.hour(),
                3 => when.month0(),
                _ => (when.date_naive() - start).num_days() as u32,
            };
            if let Some(c) = bars.get_mut(&key) {
                c.add(&row.counts);
            }
        } else if when.date_naive() >= previous {
            old.add(&row.counts);
        }
    }
    let view = |c: &Counts| {
        json!({"chinese":c.chinese,"english":c.english_words,"chineseMs":c.chinese_ms,"englishMs":c.english_word_ms,
        "chineseSpeed":speed(c.chinese,c.chinese_ms,10),"englishSpeed":speed(c.english_words,c.english_word_ms,5)})
    };
    Ok(
        json!({"input":{"chinese":current.input.grouped(true),"english":current.input.grouped(false),"cancelled":current.input.cancelled},"versions":versions,"current":view(&current),"previous":view(&old),"recent":view(&recent),"voice":view(&voice),"bars":bars.iter().map(|(i,c)|json!({"label":match period{0=>format!("{i}"),3=>format!("{}月",i+1),_=>{let d=start+chrono::Duration::days(*i as i64);format!("{}/{}",d.month(),d.day())}},"counts":view(c)})).collect::<Vec<_>>()}),
    )
}
fn speed(n: u64, ms: u64, min: u64) -> Option<u64> {
    (n >= min && ms >= 10000).then(|| (n as f64 * 60000. / ms as f64).round() as u64)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mac_keyboard_voice_and_mobile_history_stay_separate() {
        let make = |stream: &str, n: u64| Bucket {
            device: "a".repeat(32),
            version: String::new(),
            stream: stream.into(),
            minute: Local::now().timestamp() as u64 / 60,
            counts: Counts {
                chinese: n,
                english_words: n / 2,
                chinese_ms: 60000,
                english_word_ms: 30000,
                ..Default::default()
            },
        };
        let rows = vec![
            make("macos.log", 20),
            make("macos-voice.log", 10),
            make("android.log", 999),
            make("desktop-other.log", 777),
        ];
        for period in 0..4 {
            let result = summary(&rows, period, None).unwrap_or_default();
            assert_eq!(result["current"]["chinese"], 20);
            assert_eq!(result["current"]["english"], 10);
            assert_eq!(result["current"]["englishSpeed"], 20);
            assert_eq!(result["voice"]["chinese"], 10);
        }
    }
    #[test]
    fn word_counts_split_across_commits_backspace_and_focus_boundary() {
        let mut a = Activity::default();
        assert_eq!(a.commit("hel").english_words, 0);
        a.backspace();
        assert_eq!(a.commit("lo ").english_words, 1);
        assert_eq!(a.commit("GPT4").english_words, 0);
        assert_eq!(a.boundary().english_words, 1);
        assert_eq!(a.boundary().english_words, 0);
        assert_eq!(a.commit("123 ，").english_words, 0);
        assert_eq!(a.commit("你好").chinese, 2);
        assert_eq!(speed(1, 100000, 5), None);
    }
}

#[cfg(test)]
mod version_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn version_filter_applies_to_speed_bars_and_voice_and_defaults_to_all() {
        let mut rows = Vec::new();
        for (version, n) in [("", 10), ("0.8.0", 20), ("0.9.0", 60)] {
            for base in ["macos.log", "macos-voice.log"] {
                rows.push(Bucket {
                    device: "a".repeat(32),
                    stream: if version.is_empty() {
                        base.into()
                    } else {
                        retype_sync::statistics::versioned_stream(base, version)
                    },
                    version: version.into(),
                    minute: Local::now().timestamp() as u64 / 60,
                    counts: Counts {
                        chinese: n,
                        chinese_ms: 10_000,
                        ..Default::default()
                    },
                });
            }
        }
        for period in 0..4 {
            let all = summary(&rows, period, None).unwrap();
            assert_eq!(all["current"]["chinese"], 90);
            assert_eq!(all["current"]["chineseSpeed"], 180);
            assert_eq!(all["voice"]["chinese"], 90);
            assert_eq!(all["versions"].as_array().unwrap().len(), 3);
            let old = summary(&rows, period, Some("0.8.0")).unwrap();
            assert_eq!(old["current"]["chinese"], 20);
            assert_eq!(old["recent"]["chineseSpeed"], 120);
            assert_eq!(old["voice"]["chinese"], 20);
            assert_eq!(
                old["bars"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v["counts"]["chinese"].as_u64().unwrap())
                    .sum::<u64>(),
                20
            );
            assert_eq!(
                summary(&rows, period, Some("")).unwrap()["current"]["chinese"],
                10
            );
        }
    }
}
