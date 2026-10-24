use crate::Result;
pub use retype_types::statistics::{InputStats, LogRecord, Totals as Counts};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read},
    path::Path,
};
pub fn parse_record(line: &str) -> Option<LogRecord> {
    if line.starts_with('{') {
        serde_json::from_str(line).ok()
    } else {
        LogRecord::parse(line)
    }
}
pub fn valid_version(v: &str) -> bool {
    v.len() <= 64
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
}
/// Encoding the version in the stream also preserves attribution when an older
/// client forwards buckets without the new version field.
pub fn versioned_stream(base: &str, version: &str) -> String {
    format!(
        "{}@{version}.log",
        base.strip_suffix(".log").unwrap_or(base)
    )
}
pub fn stream_version(stream: &str) -> &str {
    stream
        .strip_suffix(".log")
        .and_then(|s| s.rsplit_once('@'))
        .map_or("", |(_, v)| v)
}
pub fn is_stream(stream: &str, base: &str) -> bool {
    stream == base
        || stream
            .strip_suffix(".log")
            .and_then(|s| s.rsplit_once('@'))
            .is_some_and(|(s, _)| Some(s) == base.strip_suffix(".log"))
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    pub device: String,
    pub stream: String,
    pub minute: u64,
    pub counts: Counts,
    #[serde(default)]
    pub version: String,
}
pub fn capture(root: &Path, device: &str) -> Result<Vec<Bucket>> {
    let mut result = Vec::new();
    if !root.exists() {
        return Ok(result);
    }
    let reset = std::fs::read_to_string(root.join("reset.txt"))
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    for entry in std::fs::read_dir(root).map_err(|_| "无法读取打字统计")? {
        let entry = entry.map_err(|_| "无法读取打字统计文件")?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "log")
            || !entry.file_type().map_err(|_| "统计文件类型无效")?.is_file()
        {
            continue;
        }
        let stream = path
            .file_name()
            .ok_or("统计文件名无效")?
            .to_string_lossy()
            .into_owned();
        let mut minutes = BTreeMap::<(u64, String), Counts>::new();
        let mut reader =
            BufReader::new(std::fs::File::open(&path).map_err(|_| "无法读取打字统计文件")?);
        loop {
            let mut line = String::new();
            let read = reader
                .by_ref()
                .take(4097)
                .read_line(&mut line)
                .map_err(|_| "统计文件格式无效")?;
            if line.len() > 4096 {
                return Err("统计记录过长".into());
            }
            if read == 0 || !line.ends_with('\n') {
                break;
            }
            let Some(mut row) = parse_record(line.trim()) else {
                continue;
            };
            if row.version.is_empty() {
                row.version = stream_version(&stream).to_owned();
            }
            if row.second.saturating_mul(1000) < reset || !valid_version(&row.version) {
                continue;
            }
            minutes
                .entry((row.second / 60, row.version))
                .or_default()
                .add(&row.counts);
        }
        for ((minute, version), counts) in minutes {
            result.push(Bucket {
                device: device.into(),
                stream: stream.clone(),
                minute,
                counts,
                version,
            });
        }
    }
    result.sort_by(|a, b| {
        (&a.device, &a.stream, a.minute, &a.version)
            .cmp(&(&b.device, &b.stream, b.minute, &b.version))
    });
    Ok(result)
}
pub fn merge(existing: &mut Vec<Bucket>, incoming: &[Bucket]) -> Result<()> {
    if incoming.len() > 200_000 {
        return Err("云端统计记录超过限制".into());
    }
    let mut all = BTreeMap::new();
    for row in existing.iter().chain(incoming) {
        let mut normalized = row.clone();
        if normalized.version.is_empty() {
            normalized.version = stream_version(&normalized.stream).to_owned();
        }
        let row = &normalized;
        if !crate::config::valid_id(&row.device)
            || !valid_version(&row.version)
            || row.stream.len() > 128
            || !row.stream.ends_with(".log")
            || row.minute > 50_000_000
            || [
                row.counts.chinese,
                row.counts.english,
                row.counts.chinese_ms,
                row.counts.english_ms,
                row.counts.english_words,
                row.counts.english_word_ms,
            ]
            .into_iter()
            .chain(row.counts.input.values())
            .any(|v| v > 1_000_000_000_000_000)
        {
            return Err("云端统计记录无效".into());
        }
        let entry = all
            .entry((
                row.device.clone(),
                row.stream.clone(),
                row.minute,
                row.version.clone(),
            ))
            .or_insert_with(|| row.clone());
        entry.counts.merge_max(&row.counts);
    }
    *existing = all.into_values().collect();
    Ok(())
}
/// Remote sources plus any evidence missing from this device's local logs.
/// Do not re-export these differences as new typing activity.
pub fn history(imported: &[Bucket], local: &[Bucket], device: &str) -> Vec<Bucket> {
    let available: BTreeMap<_, _> = local
        .iter()
        .map(|row| {
            (
                (row.stream.as_str(), row.minute, row.version.as_str()),
                &row.counts,
            )
        })
        .collect();
    imported
        .iter()
        .filter_map(|row| {
            if row.device != device {
                return Some(row.clone());
            }
            let mut missing = row.clone();
            if let Some(counts) =
                available.get(&(row.stream.as_str(), row.minute, row.version.as_str()))
            {
                missing.counts.subtract(counts);
            }
            (missing.counts != Counts::default()).then_some(missing)
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn word_fields_survive_legacy_merges_and_local_restore() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("retype-sync-word-stats-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        std::fs::write(
            root.join("typing.log"),
            "3600,2,100,400,60000\n3601,0,10,0,500,2,500\n",
        )
        .map_err(|e| e.to_string())?;
        let local = capture(&root, &"a".repeat(32))?;
        assert_eq!(local[0].counts.english_words, 2);
        assert_eq!(local[0].counts.english_word_ms, 500);
        let old: Vec<Bucket> = serde_json::from_str(&format!(r#"[{{"device":"{}","stream":"typing.log","minute":60,"counts":{{"chinese":2,"english":110,"chinese_ms":400,"english_ms":60500}}}}]"#, "a".repeat(32))).map_err(|e| e.to_string())?;
        let mut combined = local.clone();
        merge(&mut combined, &old)?;
        merge(&mut combined, &local)?;
        assert_eq!(combined, local);
        assert!(history(&combined, &local, &"a".repeat(32)).is_empty());
        let restored = history(&combined, &old, &"a".repeat(32));
        assert_eq!(restored[0].counts.english_words, 2);
        assert_eq!(restored[0].counts.english_word_ms, 500);
        assert_eq!(restored[0].counts.english, 0);
        std::fs::remove_dir_all(root).map_err(|e| e.to_string())?;
        Ok(())
    }
    #[test]
    fn repeated_downloads_use_absolute_counts_and_partial_lines_wait() -> Result<()> {
        let root = std::env::temp_dir().join(format!("retype-sync-stats-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let path = root.join("1-123.log");
        std::fs::write(&path, "3600,2,3,400,500\n3601,1,2,100,200\n3602,99")
            .map_err(|e| e.to_string())?;
        let rows = capture(&root, &"a".repeat(32))?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].counts.chinese, 3);
        let mut combined = vec![];
        merge(&mut combined, &rows)?;
        merge(&mut combined, &rows)?;
        assert_eq!(combined, rows);
        assert!(history(&combined, &rows, &"a".repeat(32)).is_empty());
        let mut restored = rows.clone();
        restored[0].counts.chinese = 1;
        assert_eq!(
            history(&combined, &restored, &"a".repeat(32))[0]
                .counts
                .chinese,
            2
        );
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        std::fs::remove_dir(&root).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod version_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn versions_and_new_measurements_survive_old_relays_and_repeated_restore() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("retype-version-sync-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let mut counts = Counts {
            chinese: 2,
            ..Default::default()
        };
        counts.input.record(true, 2, true, 2);
        let record = LogRecord {
            second: 3600,
            version: "0.8.0".into(),
            counts,
        };
        let stream = versioned_stream("typing.log", "0.8.0");
        std::fs::write(
            root.join(&stream),
            format!(
                "{}\n3601,1,0,0,0\n",
                serde_json::to_string(&record).unwrap()
            ),
        )
        .map_err(|e| e.to_string())?;
        let local = capture(&root, &"a".repeat(32))?;
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].counts.chinese, 3);
        let mut combined = local.clone();
        let mut old = local.clone();
        old[0].version.clear();
        old[0].counts.input = InputStats::default();
        merge(&mut combined, &old)?;
        merge(&mut combined, &local)?;
        assert_eq!(combined, local);
        assert!(history(&combined, &local, &"a".repeat(32)).is_empty());
        // Restoring missing metric fields must not restore the old character count twice.
        old[0].version = "0.8.0".into();
        let missing = history(&combined, &old, &"a".repeat(32));
        assert_eq!(missing[0].counts.chinese, 0);
        assert_eq!(missing[0].counts.input.total(true).corrected, 1);
        let mut newer = local.clone();
        newer[0].version = "0.9.0".into();
        merge(&mut combined, &newer)?;
        assert_eq!(combined.len(), 2);
        let mut bad = local.clone();
        bad[0].version = "../invalid".into();
        assert!(merge(&mut combined, &bad).is_err());
        std::fs::remove_dir_all(root).map_err(|e| e.to_string())?;
        Ok(())
    }
}
