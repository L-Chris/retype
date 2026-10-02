use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read},
    path::Path,
};
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub chinese: u64,
    pub english: u64,
    pub chinese_ms: u64,
    pub english_ms: u64,
}
impl Counts {
    pub fn add(&mut self, other: &Self) {
        self.chinese = self.chinese.saturating_add(other.chinese);
        self.english = self.english.saturating_add(other.english);
        self.chinese_ms = self.chinese_ms.saturating_add(other.chinese_ms);
        self.english_ms = self.english_ms.saturating_add(other.english_ms);
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    pub device: String,
    pub stream: String,
    pub minute: u64,
    pub counts: Counts,
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
        let mut minutes = BTreeMap::<u64, Counts>::new();
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
            let values: Option<Vec<u64>> = line.trim().split(',').map(|v| v.parse().ok()).collect();
            let Some(v) = values.filter(|v| v.len() == 5) else {
                continue;
            };
            if v[0].saturating_mul(1000) < reset {
                continue;
            }
            minutes.entry(v[0] / 60).or_default().add(&Counts {
                chinese: v[1],
                english: v[2],
                chinese_ms: v[3],
                english_ms: v[4],
            });
        }
        let stream = path
            .file_name()
            .ok_or("统计文件名无效")?
            .to_string_lossy()
            .into_owned();
        for (minute, counts) in minutes {
            result.push(Bucket {
                device: device.into(),
                stream: stream.clone(),
                minute,
                counts,
            });
        }
    }
    result.sort_by(|a, b| (&a.device, &a.stream, a.minute).cmp(&(&b.device, &b.stream, b.minute)));
    Ok(result)
}
pub fn merge(existing: &mut Vec<Bucket>, incoming: &[Bucket]) -> Result<()> {
    if incoming.len() > 200_000 {
        return Err("云端统计记录超过限制".into());
    }
    let mut all = BTreeMap::new();
    for row in existing.iter().chain(incoming) {
        if !crate::config::valid_id(&row.device)
            || row.stream.len() > 128
            || !row.stream.ends_with(".log")
            || row.minute > 50_000_000
        {
            return Err("云端统计记录无效".into());
        }
        let entry = all
            .entry((row.device.clone(), row.stream.clone(), row.minute))
            .or_insert_with(|| row.clone());
        entry.counts.chinese = entry.counts.chinese.max(row.counts.chinese);
        entry.counts.english = entry.counts.english.max(row.counts.english);
        entry.counts.chinese_ms = entry.counts.chinese_ms.max(row.counts.chinese_ms);
        entry.counts.english_ms = entry.counts.english_ms.max(row.counts.english_ms);
    }
    *existing = all.into_values().collect();
    Ok(())
}
/// Remote sources plus any evidence missing from this device's local logs.
/// Do not re-export these differences as new typing activity.
pub fn history(imported: &[Bucket], local: &[Bucket], device: &str) -> Vec<Bucket> {
    let available: BTreeMap<_, _> = local
        .iter()
        .map(|row| ((row.stream.as_str(), row.minute), &row.counts))
        .collect();
    imported
        .iter()
        .filter_map(|row| {
            if row.device != device {
                return Some(row.clone());
            }
            let mut missing = row.clone();
            if let Some(counts) = available.get(&(row.stream.as_str(), row.minute)) {
                missing.counts.chinese = missing.counts.chinese.saturating_sub(counts.chinese);
                missing.counts.english = missing.counts.english.saturating_sub(counts.english);
                missing.counts.chinese_ms =
                    missing.counts.chinese_ms.saturating_sub(counts.chinese_ms);
                missing.counts.english_ms =
                    missing.counts.english_ms.saturating_sub(counts.english_ms);
            }
            (missing.counts != Counts::default()).then_some(missing)
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
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
