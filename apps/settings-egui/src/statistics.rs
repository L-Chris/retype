//! Reader for the existing numeric TSF logs. No input text is stored here.
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::PathBuf,
    time::SystemTime,
};

pub use retype_sync::statistics::Counts;
fn voice_stream(stream: &str) -> bool {
    stream.starts_with("voice-") || retype_sync::statistics::is_stream(stream, "macos-voice.log")
}
#[cfg(test)]
pub fn speed(count: u64, active_ms: u64) -> Option<u64> {
    retype_types::statistics::speed(count, active_ms, 10)
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Period {
    Day,
    Week,
    Month,
    Year,
}
impl Period {
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn label(self) -> &'static str {
        match self {
            Self::Day => "天",
            Self::Week => "周",
            Self::Month => "月",
            Self::Year => "年",
        }
    }
    fn start(self, date: NaiveDate) -> NaiveDate {
        match self {
            Self::Day => date,
            Self::Week => date - Duration::days(date.weekday().num_days_from_monday().into()),
            Self::Month => date.with_day(1).unwrap_or(date),
            Self::Year => date
                .with_day(1)
                .and_then(|d| d.with_month(1))
                .unwrap_or(date),
        }
    }
    fn shift(self, date: NaiveDate, offset: i32) -> NaiveDate {
        match self {
            Self::Day => date + Duration::days(offset.into()),
            Self::Week => date + Duration::days(i64::from(offset) * 7),
            Self::Month => {
                let month = date.year() * 12 + date.month0() as i32 + offset;
                NaiveDate::from_ymd_opt(month.div_euclid(12), month.rem_euclid(12) as u32 + 1, 1)
                    .unwrap_or(date)
            }
            Self::Year => date.with_year(date.year() + offset).unwrap_or(date),
        }
    }
}
#[derive(Default, Clone)]
pub struct Snapshot {
    pub today: Counts,
    pub total: Counts,
    pub voice: Counts,
    pub chinese_speed: Option<u64>,
    pub english_speed: Option<u64>,
    pub days: BTreeMap<NaiveDate, Counts>,
    pub by_version: BTreeMap<String, Snapshot>,
}
impl Snapshot {
    pub fn history(&self, today: NaiveDate, period: Period) -> Vec<(NaiveDate, Counts)> {
        let length = match period {
            Period::Day => 7,
            Period::Week => 8,
            Period::Month => 12,
            Period::Year => 5,
        };
        let current = period.start(today);
        (1 - length..=0)
            .map(|offset| {
                let start = period.shift(current, offset);
                let mut counts = Counts::default();
                for (day, value) in &self.days {
                    if period.start(*day) == start {
                        counts.add(value);
                    }
                }
                (start, counts)
            })
            .collect()
    }
}
struct Cache {
    size: u64,
    modified: Option<SystemTime>,
    offset: u64,
    days: BTreeMap<(String, NaiveDate), Counts>,
    recent: Vec<(i64, String, Counts)>,
}
pub struct Store {
    root: PathBuf,
    files: BTreeMap<PathBuf, Cache>,
    reset: i64,
}
impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            files: BTreeMap::new(),
            reset: 0,
        }
    }
    pub fn load(&mut self, now: i64) -> std::io::Result<Snapshot> {
        let reset = fs::read_to_string(self.root.join("reset.txt"))
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        if reset != self.reset {
            self.files.clear();
            self.reset = reset;
        }
        if !self.root.exists() {
            return Ok(Snapshot::default());
        }
        let mut seen = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|v| v != "log") || !entry.file_type()?.is_file() {
                continue;
            }
            seen.push(path.clone());
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let modified = metadata.modified().ok();
            let mut cache = self
                .files
                .remove(&path)
                .filter(|cache| {
                    metadata.len() >= cache.size
                        && (metadata.len() > cache.size || modified == cache.modified)
                })
                .unwrap_or(Cache {
                    size: 0,
                    modified: None,
                    offset: 0,
                    days: BTreeMap::new(),
                    recent: Vec::new(),
                });
            cache.recent.retain(|(time, _, _)| *time >= now - 300_000);
            if metadata.len() != cache.size
                || modified != cache.modified
                || cache.offset < metadata.len()
            {
                if let Ok(mut file) = fs::File::open(&path) {
                    file.seek(SeekFrom::Start(cache.offset))?;
                    let mut reader = BufReader::new(file);
                    let mut line = Vec::new();
                    loop {
                        line.clear();
                        let count = reader.read_until(b'\n', &mut line)?;
                        if count == 0 || line.last() != Some(&b'\n') {
                            break;
                        }
                        cache.offset += count as u64;
                        if line.len() > 4096 {
                            continue;
                        }
                        let text = String::from_utf8_lossy(&line);
                        let Some(mut row) = retype_sync::statistics::parse_record(text.trim())
                        else {
                            continue;
                        };
                        if row.version.is_empty() {
                            row.version = retype_sync::statistics::stream_version(
                                &path.file_name().unwrap_or_default().to_string_lossy(),
                            )
                            .to_owned();
                        }
                        if !retype_sync::statistics::valid_version(&row.version) {
                            continue;
                        }
                        let Some(time) = row
                            .second
                            .checked_mul(1000)
                            .and_then(|v| i64::try_from(v).ok())
                        else {
                            continue;
                        };
                        if time < reset || time > now {
                            continue;
                        }
                        let Some(date) = Local
                            .timestamp_millis_opt(time)
                            .single()
                            .map(|v| v.date_naive())
                        else {
                            continue;
                        };
                        let counts = row.counts;
                        cache
                            .days
                            .entry((row.version.clone(), date))
                            .or_default()
                            .add(&counts);
                        if time >= now - 300_000 {
                            cache.recent.push((time, row.version, counts));
                        }
                    }
                }
                cache.size = metadata.len();
                cache.modified = modified;
            }
            self.files.insert(path, cache);
        }
        self.files.retain(|path, _| seen.contains(path));
        let mut result = Snapshot::default();
        let mut recent = BTreeMap::<String, (Counts, i64, i64)>::new();
        for (path, cache) in &self.files {
            let voice = path
                .file_name()
                .is_some_and(|name| voice_stream(&name.to_string_lossy()));
            for ((version, day), counts) in &cache.days {
                let target = result.by_version.entry(version.clone()).or_default();
                if voice {
                    target.voice.add(counts);
                } else {
                    target.days.entry(*day).or_default().add(counts);
                }
            }
            if voice {
                continue;
            }
            for (time, version, counts) in &cache.recent {
                let entry = recent.entry(version.clone()).or_default();
                entry.0.add(counts);
                if counts.chinese > 0 {
                    entry.1 = entry.1.max(*time);
                }
                if counts.english_words > 0 {
                    entry.2 = entry.2.max(*time);
                }
            }
        }
        if let Ok(bytes) = fs::read(self.root.join("cloud-history.json")) {
            if let Ok(rows) = serde_json::from_slice::<Vec<retype_sync::statistics::Bucket>>(&bytes)
            {
                for mut row in rows {
                    if row.version.is_empty() {
                        row.version =
                            retype_sync::statistics::stream_version(&row.stream).to_owned();
                    }
                    // Preserve Android rows in sync storage, excluding them from desktop totals.
                    if row.stream.starts_with("android") {
                        continue;
                    }
                    let Some(seconds) = row
                        .minute
                        .checked_mul(60)
                        .and_then(|v| i64::try_from(v).ok())
                    else {
                        continue;
                    };
                    if seconds.saturating_mul(1000) < reset || seconds.saturating_mul(1000) > now {
                        continue;
                    }
                    let target = result.by_version.entry(row.version).or_default();
                    if voice_stream(&row.stream) {
                        target.voice.add(&row.counts);
                        continue;
                    }
                    if let Some(date) = Local
                        .timestamp_opt(seconds, 0)
                        .single()
                        .map(|v| v.date_naive())
                    {
                        target.days.entry(date).or_default().add(&row.counts);
                    }
                }
            }
        }
        let today = Local
            .timestamp_millis_opt(now)
            .single()
            .map(|v| v.date_naive());
        let mut all_recent = (Counts::default(), 0, 0);
        for (version, target) in &mut result.by_version {
            for (day, counts) in &target.days {
                target.total.add(counts);
                result.days.entry(*day).or_default().add(counts);
            }
            target.today = today
                .and_then(|d| target.days.get(&d).copied())
                .unwrap_or_default();
            result.total.add(&target.total);
            result.voice.add(&target.voice);
            if let Some((counts, zh, en)) = recent.get(version) {
                if now - *zh <= 30_000 {
                    target.chinese_speed = counts.chinese_speed();
                }
                if now - *en <= 30_000 {
                    target.english_speed = counts.english_speed();
                }
                all_recent.0.add(counts);
                all_recent.1 = all_recent.1.max(*zh);
                all_recent.2 = all_recent.2.max(*en);
            }
        }
        result.today = today
            .and_then(|d| result.days.get(&d).copied())
            .unwrap_or_default();
        if now - all_recent.1 <= 30_000 {
            result.chinese_speed = all_recent.0.chinese_speed();
        }
        if now - all_recent.2 <= 30_000 {
            result.english_speed = all_recent.0.english_speed();
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn word_speed_uses_only_new_metric_time_and_weighted_period_totals() -> std::io::Result<()> {
        let root =
            std::env::temp_dir().join(format!("retype-word-statistics-{}", std::process::id()));
        fs::create_dir_all(&root)?;
        let now = Local::now().timestamp_millis();
        fs::write(
            root.join("legacy.log"),
            format!("{},0,5000,0,600000\n", now / 1000),
        )?;
        fs::write(
            root.join("words.log"),
            format!(
                "{},0,100,0,10000,5,10000\n{},0,100,0,20000,5,20000\n",
                now / 1000,
                now / 1000
            ),
        )?;
        let mut store = Store::new(root.clone());
        let snapshot = store.load(now)?;
        assert_eq!(snapshot.total.english, 5200);
        assert_eq!(snapshot.total.english_words, 10);
        assert_eq!(snapshot.english_speed, Some(20));
        assert_eq!(snapshot.total.english_speed(), Some(20));
        assert_eq!(
            Counts {
                english_words: 4,
                english_word_ms: 60000,
                ..Default::default()
            }
            .english_speed(),
            None
        );
        assert_eq!(
            Counts {
                english_words: 5,
                english_word_ms: 9999,
                ..Default::default()
            }
            .english_speed(),
            None
        );
        assert_eq!(store.load(now + 31000)?.english_speed, None);
        fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn history_weights_activity_and_handles_calendar_boundaries() {
        let today = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap_or_default();
        let yesterday = today - Duration::days(1);
        let mut snapshot = Snapshot::default();
        snapshot.days.insert(
            today,
            Counts {
                chinese: 60,
                chinese_ms: 60_000,
                ..Default::default()
            },
        );
        snapshot.days.insert(
            yesterday,
            Counts {
                chinese: 120,
                chinese_ms: 30_000,
                ..Default::default()
            },
        );
        let week = snapshot.history(today, Period::Week);
        assert_eq!(week.last().map(|(_, v)| v.chinese_speed()), Some(Some(120)));
        assert_eq!(snapshot.history(today, Period::Month).len(), 12);
        assert_eq!(snapshot.history(today, Period::Year).len(), 5);
        assert_eq!(speed(9, 60_000), None);
    }
    #[test]
    fn synced_mobile_counts_do_not_enter_desktop_statistics() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "retype-statistics-platform-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root)?;
        let now = Local::now().timestamp_millis();
        fs::write(
            root.join("local.log"),
            format!("{},20,10,10000,10000\n", now / 1000),
        )?;
        let rows = [("android.log", 1000, 2000), ("remote-desktop.log", 30, 40)].map(
            |(stream, chinese, english)| retype_sync::statistics::Bucket {
                device: "a".repeat(32),
                version: String::new(),
                stream: stream.into(),
                minute: (now / 60000) as u64,
                counts: retype_sync::statistics::Counts {
                    chinese,
                    english,
                    chinese_ms: 60000,
                    english_ms: 60000,
                    ..Default::default()
                },
            },
        );
        fs::write(root.join("cloud-history.json"), serde_json::to_vec(&rows)?)?;
        let mut store = Store::new(root.clone());
        for _ in 0..2 {
            let snapshot = store.load(now)?;
            assert_eq!(
                snapshot.total,
                Counts {
                    chinese: 50,
                    english: 50,
                    chinese_ms: 70000,
                    english_ms: 70000,
                    ..Default::default()
                }
            );
            assert_eq!(snapshot.today, snapshot.total);
            assert_eq!(
                snapshot
                    .history(Local::now().date_naive(), Period::Day)
                    .last()
                    .map(|(_, counts)| *counts),
                Some(snapshot.total)
            );
        }
        assert_eq!(
            serde_json::from_slice::<Vec<retype_sync::statistics::Bucket>>(&fs::read(
                root.join("cloud-history.json")
            )?)?
            .len(),
            2
        );
        fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn append_partial_reset_and_idle_are_compatible_with_tsf_logs() -> std::io::Result<()> {
        let root =
            std::env::temp_dir().join(format!("retype-statistics-test-{}", std::process::id()));
        fs::create_dir_all(&root)?;
        let now = Local::now().timestamp_millis();
        let time = now / 1000;
        let file = root.join("test.log");
        fs::write(&file, format!("{time},20,10,10000,10000\n{time},5"))?;
        let mut store = Store::new(root.clone());
        assert_eq!(store.load(now)?.total.total(), 30);
        fs::write(
            &file,
            format!("{time},20,10,10000,10000\n{time},5,0,5000,0\n"),
        )?;
        assert_eq!(store.load(now)?.total.total(), 35);
        assert_eq!(store.load(now)?.total.total(), 35);
        assert_eq!(store.load(now + 31_000)?.chinese_speed, None);
        fs::write(root.join("reset.txt"), (now + 1).to_string())?;
        fs::write(&file, format!("{time},20,10,10000,10000\n"))?;
        assert_eq!(store.load(now + 1)?.total.total(), 0);
        fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn voice_counts_do_not_enter_keyboard_totals_or_speed() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "retype-voice-statistics-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root)?;
        let now = chrono::Local::now().timestamp_millis();
        let second = now / 1000;
        fs::write(
            root.join("keyboard.log"),
            format!("{second},3,0,15000,0,2,15000\n"),
        )?;
        fs::write(
            root.join("voice-synthetic.log"),
            format!("{second},500,0,0,0,200,0\n"),
        )?;
        let mut store = Store::new(root.clone());
        let snapshot = store.load(now)?;
        assert_eq!(snapshot.total.chinese, 3);
        assert_eq!(snapshot.total.english_words, 2);
        assert_eq!(snapshot.voice.chinese, 500);
        assert_eq!(snapshot.voice.english_words, 200);
        assert_eq!(snapshot.chinese_speed, None);
        assert_eq!(snapshot.english_speed, None);
        fs::remove_file(root.join("keyboard.log"))?;
        fs::remove_file(root.join("voice-synthetic.log"))?;
        fs::remove_dir(root)?;
        Ok(())
    }
}

#[cfg(test)]
mod version_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn all_versions_and_filtered_history_weight_raw_counts_and_keep_legacy_unknown(
    ) -> std::io::Result<()> {
        let root =
            std::env::temp_dir().join(format!("retype-version-statistics-{}", std::process::id()));
        fs::create_dir_all(&root)?;
        let now = Local::now().timestamp_millis();
        let mut lines = String::new();
        for (version, words) in [("0.8.0", 5), ("0.9.0", 20)] {
            let mut counts = Counts {
                english_words: words,
                english_word_ms: 10_000,
                ..Default::default()
            };
            counts.input.record(true, 2, version == "0.8.0", 1);
            lines.push_str(&serde_json::to_string(
                &retype_sync::statistics::LogRecord {
                    second: (now / 1000) as u64,
                    version: version.into(),
                    counts,
                },
            )?);
            lines.push('\n');
        }
        lines.push_str(&format!("{},0,5000,0,600000\n", now / 1000));
        fs::write(root.join("typing.log"), lines)?;
        let mut store = Store::new(root.clone());
        let all = store.load(now)?;
        assert_eq!(all.by_version.len(), 3);
        assert_eq!(all.total.english_speed(), Some(75));
        assert_eq!(all.total.input.total(true).inputs, 2);
        assert_eq!(all.total.input.total(true).corrected, 1);
        assert_eq!(all.by_version["0.8.0"].total.english_speed(), Some(30));
        assert_eq!(all.by_version["0.9.0"].english_speed, Some(120));
        assert_eq!(all.by_version[""].total.english, 5000);
        assert_eq!(all.by_version[""].total.input, Default::default());
        let today = Local::now().date_naive();
        for period in [Period::Day, Period::Week, Period::Month, Period::Year] {
            assert_eq!(
                all.history(today, period).last().unwrap().1.english_speed(),
                Some(75)
            );
            assert_eq!(
                all.by_version["0.8.0"]
                    .history(today, period)
                    .last()
                    .unwrap()
                    .1
                    .input
                    .total(true)
                    .corrected,
                1
            );
        }
        assert_eq!(store.load(now)?.total, all.total);
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
