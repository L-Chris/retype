//! Immutable, independently downloaded system dictionaries. Never called by key handlers.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
pub mod packs;
pub type Result<T> = std::result::Result<T, String>;
pub const BUNDLE_MAGIC: &[u8; 8] = b"RTRES001";
static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Component {
    pub id: String,
    pub format: String,
    pub bytes: u64,
    pub sha256: String,
    pub compressed_bytes: u64,
    pub compressed_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema: u32,
    pub components: Vec<Component>,
}
impl Catalog {
    pub fn builtin() -> Result<Self> {
        let catalog: Self = serde_json::from_str(include_str!("../../../data/dict/resources.json"))
            .map_err(|_| "基础资源清单无效")?;
        catalog.validate()?;
        Ok(catalog)
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 || self.components.len() != 2 {
            return Err("基础资源清单版本不兼容".into());
        }
        for (component, (id, format)) in self
            .components
            .iter()
            .zip([("base", "RTDICT02"), ("english", "RTENGL01")])
        {
            if component.id != id
                || component.format != format
                || !(24..=256 * 1024 * 1024).contains(&component.bytes)
                || !(1..=128 * 1024 * 1024).contains(&component.compressed_bytes)
                || !hash_name(&component.sha256)
                || !hash_name(&component.compressed_sha256)
                || component
                    .release
                    .as_ref()
                    .is_some_and(|v| !valid_release(v))
            {
                return Err("基础资源清单无效".into());
            }
        }
        Ok(())
    }
}
fn valid_release(value: &str) -> bool {
    value.starts_with('v')
        && value[1..].split('.').count() == 3
        && value[1..]
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
}
fn hash_name(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl Component {
    pub fn release_tag(&self) -> String {
        self.release
            .clone()
            .unwrap_or_else(|| format!("v{}", env!("CARGO_PKG_VERSION")))
    }
    pub fn release_url(&self) -> String {
        format!(
            "https://api.github.com/repos/L-Chris/retype/releases/tags/{}",
            self.release_tag()
        )
    }
    /// Select only the pinned resource from the official release API response.
    pub fn download_url(&self, release: &serde_json::Value) -> Result<String> {
        if release["tag_name"] != self.release_tag()
            || release["draft"].as_bool().unwrap_or(false)
            || release["prerelease"].as_bool().unwrap_or(false)
        {
            return Err("词库发布信息无效".into());
        }
        let asset = release["assets"]
            .as_array()
            .and_then(|assets| assets.iter().find(|a| a["name"] == self.asset()))
            .ok_or("当前版本缺少词库附件")?;
        let id = asset["id"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or("词库附件无效")?;
        let url = format!("https://api.github.com/repos/L-Chris/retype/releases/assets/{id}");
        if asset["url"] != url
            || asset["size"].as_u64() != Some(self.compressed_bytes)
            || asset["state"] != "uploaded"
            || asset["digest"]
                .as_str()
                .is_some_and(|digest| digest != format!("sha256:{}", self.compressed_sha256))
        {
            return Err("词库附件信息与校验清单不一致".into());
        }
        Ok(url)
    }
    pub fn asset(&self) -> String {
        format!("retype-{}-{}.bin.gz", self.id, self.sha256)
    }
    pub fn url(&self) -> String {
        format!(
            "https://github.com/L-Chris/retype/releases/download/{}/{}",
            self.release_tag(),
            self.asset()
        )
    }
    pub fn path(&self, root: &Path) -> PathBuf {
        root.join(format!("{}-{}.bin", self.id, self.sha256))
    }
    pub fn partial(&self, root: &Path) -> PathBuf {
        root.join("downloads")
            .join(format!("{}-{}.part", self.id, self.compressed_sha256))
    }
    pub fn present(&self, root: &Path) -> bool {
        fs::metadata(self.path(root)).is_ok_and(|m| {
            m.is_file()
                && m.len() == self.bytes
                && (!cfg!(any(target_os = "android", target_os = "macos"))
                    || m.permissions().readonly())
        })
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub ready: bool,
    pub base: Option<PathBuf>,
    pub english: Option<PathBuf>,
    pub missing: Vec<String>,
    pub download_bytes: u64,
}
pub fn status(root: &Path) -> Result<Status> {
    let catalog = Catalog::builtin()?;
    let missing: Vec<_> = catalog
        .components
        .iter()
        .filter(|c| !c.present(root))
        .map(|c| c.id.clone())
        .collect();
    let base = available_component(root, &catalog.components[0]);
    let english = available_component(root, &catalog.components[1]);
    Ok(Status {
        ready: missing.is_empty(),
        base,
        english,
        download_bytes: catalog
            .components
            .iter()
            .filter(|c| missing.contains(&c.id))
            .map(|c| c.compressed_bytes)
            .sum(),
        missing,
    })
}
fn available_component(root: &Path, component: &Component) -> Option<PathBuf> {
    if component.present(root) {
        return Some(component.path(root));
    }
    let hash = fs::read_to_string(root.join(format!("legacy-{}.current", component.id))).ok()?;
    let hash = hash.trim();
    if !hash_name(hash) {
        return None;
    }
    [
        root.join(format!("{}-{hash}.bin", component.id)),
        root.join(format!("legacy-{}-{hash}.bin", component.id)),
    ]
    .into_iter()
    .find(|p| {
        fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() >= 24 && m.len() <= 256 * 1024 * 1024)
    })
}
pub fn activate(root: &Path) -> Result<Status> {
    let state = status(root)?;
    if let Some(path) = &state.english {
        retype_english::install(path)?;
    }
    Ok(state)
}
pub fn checksum(path: &Path) -> Result<String> {
    let mut input = File::open(path).map_err(|e| e.to_string())?;
    hash_reader(&mut input)
}
fn hash_reader(input: &mut impl Read) -> Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hex(&hash.finalize()))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
struct Lease(File);
impl Lease {
    fn acquire(root: &Path) -> Result<Self> {
        fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("install.lock"))
            .map_err(|e| e.to_string())?;
        fs2::FileExt::try_lock_exclusive(&file).map_err(|_| "另一个词库安装任务正在运行")?;
        Ok(Self(file))
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}
fn staging(root: &Path, id: &str) -> PathBuf {
    root.join(format!(
        ".{id}-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
fn validate(path: &Path, id: &str) -> Result<()> {
    if id == "base" {
        retype_dict::binary::open_shared(path)
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else if id == "english" {
        retype_english::Vocabulary::open(path).map(|_| ())
    } else {
        Err("未知基础资源".into())
    }
}
fn publish(source: &Path, destination: &Path, id: &str) -> Result<()> {
    let mut permissions = fs::metadata(source)
        .map_err(|e| e.to_string())?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(source, permissions).map_err(|e| e.to_string())?;
    validate(source, id)?;
    if destination.exists() {
        fs::rename(
            destination,
            staging(destination.parent().ok_or("资源目录无效")?, "obsolete"),
        )
        .map_err(|e| e.to_string())?;
    }
    fs::rename(source, destination).map_err(|e| e.to_string())
}
pub fn migrate(root: &Path, legacy: &[PathBuf]) -> Result<Status> {
    let _lease = Lease::acquire(root)?;
    let catalog = Catalog::builtin()?;
    for component in &catalog.components {
        if available_component(root, component).is_some() {
            continue;
        }
        let mut candidates = if component.id == "base" {
            legacy.to_vec()
        } else {
            Vec::new()
        };
        // Resources from a previous catalog are format-compatible and already immutable.
        let mut cached: Vec<_> = fs::read_dir(root)
            .map_err(|e| e.to_string())?
            .flatten()
            .take(128)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|name| {
                        name.strip_prefix(&format!("{}-", component.id))
                            .and_then(|s| s.strip_suffix(".bin"))
                            .is_some_and(hash_name)
                    })
            })
            .collect();
        cached.sort();
        candidates.extend(cached);
        for old in candidates {
            if !fs::metadata(&old)
                .is_ok_and(|m| m.is_file() && m.len() >= 24 && m.len() <= 256 * 1024 * 1024)
                || validate(&old, &component.id).is_err()
            {
                continue;
            }
            let hash = checksum(&old)?;
            let cached_path = root.join(format!("{}-{hash}.bin", component.id));
            let destination = if hash == component.sha256 {
                component.path(root)
            } else {
                root.join(format!("legacy-{}-{hash}.bin", component.id))
            };
            if old != cached_path {
                let temp = staging(root, "migration");
                let result: Result<()> = (|| {
                    let mut output = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&temp)
                        .map_err(|e| e.to_string())?;
                    std::io::copy(
                        &mut File::open(&old).map_err(|e| e.to_string())?,
                        &mut output,
                    )
                    .map_err(|e| e.to_string())?;
                    output.sync_all().map_err(|e| e.to_string())?;
                    drop(output);
                    if checksum(&temp)? != hash {
                        return Err("旧词库迁移校验失败".into());
                    }
                    publish(&temp, &destination, &component.id)
                })();
                if result.is_err() {
                    let _ = fs::remove_file(temp);
                    result?;
                }
            }
            if hash != component.sha256 {
                atomic_write(
                    &root.join(format!("legacy-{}.current", component.id)),
                    hash.as_bytes(),
                )?;
            }
            break;
        }
    }
    status(root)
}
fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let temp = staging(path.parent().ok_or("目录无效")?, "marker");
    let mut file = File::create(&temp).map_err(|e| e.to_string())?;
    file.write_all(data)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    // This marker is only created once during migration; dictionary bytes remain immutable.
    fs::rename(temp, path).map_err(|e| e.to_string())
}
pub fn install_archive(
    root: &Path,
    id: &str,
    archive: &Path,
    cancelled: &AtomicBool,
) -> Result<Status> {
    let _lease = Lease::acquire(root)?;
    let catalog = Catalog::builtin()?;
    let component = catalog
        .components
        .iter()
        .find(|c| c.id == id)
        .ok_or("未知基础资源")?;
    install_component(root, component, archive, cancelled)?;
    status(root)
}
fn install_component(
    root: &Path,
    component: &Component,
    archive: &Path,
    cancelled: &AtomicBool,
) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("下载已取消".into());
    }
    if fs::metadata(archive).map_err(|e| e.to_string())?.len() != component.compressed_bytes
        || checksum(archive)? != component.compressed_sha256
    {
        return Err("词库压缩文件校验失败".into());
    }
    if component.present(root) && checksum(&component.path(root))? == component.sha256 {
        return Ok(());
    }
    let temp = staging(root, &component.id);
    let result = (|| {
        let input = File::open(archive).map_err(|e| e.to_string())?;
        let mut input = flate2::read::GzDecoder::new(input);
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        let mut total = 0;
        let mut digest = Sha256::new();
        let mut buffer = [0; 65536];
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err("下载已取消".into());
            }
            let n = input.read(&mut buffer).map_err(|_| "词库解压失败")?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > component.bytes {
                return Err("词库解压大小超出限制".into());
            }
            output.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
            digest.update(&buffer[..n]);
        }
        output.sync_all().map_err(|e| e.to_string())?;
        drop(output);
        if total != component.bytes || hex(&digest.finalize()) != component.sha256 {
            return Err("词库内容校验失败".into());
        }
        publish(&temp, &component.path(root), &component.id)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
/// A bounded bundle has a fixed component order and no archive-supplied paths.
pub fn import_bundle(root: &Path, source: &Path, cancelled: &AtomicBool) -> Result<Status> {
    let _lease = Lease::acquire(root)?;
    let catalog = Catalog::builtin()?;
    let mut file = File::open(source).map_err(|e| e.to_string())?;
    let expected_bytes = 8 + catalog
        .components
        .iter()
        .map(|c| 8 + c.compressed_bytes)
        .sum::<u64>();
    if file.metadata().map_err(|e| e.to_string())?.len() != expected_bytes {
        return Err("词库包大小或版本不兼容".into());
    }
    let mut magic = [0; 8];
    file.read_exact(&mut magic).map_err(|_| "词库包不完整")?;
    if &magic != BUNDLE_MAGIC {
        return Err("不是 retype 词库包".into());
    }
    for component in &catalog.components {
        file.read_exact(&mut magic).map_err(|_| "词库包不完整")?;
        if u64::from_le_bytes(magic) != component.compressed_bytes {
            return Err("词库包版本不兼容".into());
        }
        let part = staging(root, "import");
        let result = (|| {
            let mut out = File::create(&part).map_err(|e| e.to_string())?;
            let n = std::io::copy(&mut (&mut file).take(component.compressed_bytes), &mut out)
                .map_err(|e| e.to_string())?;
            drop(out);
            if n != component.compressed_bytes {
                return Err("词库包不完整".into());
            }
            install_component(root, component, &part, cancelled)
        })();
        let _ = fs::remove_file(part);
        result?;
    }
    if file.read(&mut [0]).map_err(|e| e.to_string())? != 0 {
        return Err("词库包包含多余内容".into());
    }
    status(root)
}

#[cfg(feature = "download")]
mod download;
#[cfg(feature = "download")]
pub use download::download;

#[cfg(test)]
mod tests;
