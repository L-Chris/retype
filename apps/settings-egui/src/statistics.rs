//! Reader for the existing numeric TSF logs. No input text is stored here.
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::PathBuf,
    time::SystemTime,
};

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Counts {
    pub chinese: u64,
    pub english: u64,
    pub chinese_ms: u64,
    pub english_ms: u64,
}
impl Counts {
    fn add(&mut self, other: Self) {
        self.chinese = self.chinese.saturating_add(other.chinese);
        self.english = self.english.saturating_add(other.english);
        self.chinese_ms = self.chinese_ms.saturating_add(other.chinese_ms);
        self.english_ms = self.english_ms.saturating_add(other.english_ms);
    }
    pub fn total(self) -> u64 {
        self.chinese.saturating_add(self.english)
    }
    pub fn chinese_speed(self) -> Option<u64> {
        speed(self.chinese, self.chinese_ms)
    }
    pub fn english_speed(self) -> Option<u64> {
        speed(self.english, self.english_ms)
    }
}
pub fn speed(count: u64, active_ms: u64) -> Option<u64> {
    (count >= 10 && active_ms >= 10_000)
        .then(|| (count as f64 * 60_000.0 / active_ms as f64).round() as u64)
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Period {
    Day,
    Week,
    Month,
    Year,
}
impl Period {
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
    pub chinese_speed: Option<u64>,
    pub english_speed: Option<u64>,
    pub days: BTreeMap<NaiveDate, Counts>,
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
                        counts.add(*value);
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
    days: BTreeMap<NaiveDate, Counts>,
    recent: Vec<(i64, Counts)>,
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
            cache.recent.retain(|(time, _)| *time >= now - 300_000);
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
                        let values: Option<Vec<u64>> =
                            text.trim().split(',').map(|v| v.parse().ok()).collect();
                        let Some(values) = values.filter(|v| v.len() == 5) else {
                            continue;
                        };
                        let Some(time) = values[0]
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
                        let counts = Counts {
                            chinese: values[1],
                            english: values[2],
                            chinese_ms: values[3],
                            english_ms: values[4],
                        };
                        cache.days.entry(date).or_default().add(counts);
                        if time >= now - 300_000 {
                            cache.recent.push((time, counts));
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
        let mut recent = Counts::default();
        let mut last_chinese = 0;
        let mut last_english = 0;
        for cache in self.files.values() {
            for (day, counts) in &cache.days {
                result.days.entry(*day).or_default().add(*counts);
            }
            for (time, counts) in &cache.recent {
                recent.add(*counts);
                if counts.chinese > 0 {
                    last_chinese = last_chinese.max(*time);
                }
                if counts.english > 0 {
                    last_english = last_english.max(*time);
                }
            }
        }
        if let Ok(bytes) = fs::read(self.root.join("cloud-history.json")) {
            if let Ok(rows) = serde_json::from_slice::<Vec<retype_sync::statistics::Bucket>>(&bytes)
            {
                for row in rows {
                    // Preserve Android rows in sync storage, excluding them from desktop totals.
                    if row.stream == "android.log" {
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
                    if let Some(date) = Local
                        .timestamp_opt(seconds, 0)
                        .single()
                        .map(|v| v.date_naive())
                    {
                        result.days.entry(date).or_default().add(Counts {
                            chinese: row.counts.chinese,
                            english: row.counts.english,
                            chinese_ms: row.counts.chinese_ms,
                            english_ms: row.counts.english_ms,
                        });
                    }
                }
            }
        }
        for counts in result.days.values() {
            result.total.add(*counts);
        }
        if let Some(today) = Local
            .timestamp_millis_opt(now)
            .single()
            .map(|v| v.date_naive())
        {
            result.today = result.days.get(&today).copied().unwrap_or_default();
        }
        if now - last_chinese <= 30_000 {
            result.chinese_speed = recent.chinese_speed();
        }
        if now - last_english <= 30_000 {
            result.english_speed = recent.english_speed();
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
                stream: stream.into(),
                minute: (now / 60000) as u64,
                counts: retype_sync::statistics::Counts {
                    chinese,
                    english,
                    chinese_ms: 60000,
                    english_ms: 60000,
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
}
