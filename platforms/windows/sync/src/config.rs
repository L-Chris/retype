use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

pub const PROVIDERS: &[(&str, &str, &str)] = &[
    (
        "坚果云",
        "https://dav.jianguoyun.com/dav/",
        "https://help.jianguoyun.com/?tag=webdav",
    ),
    (
        "cstcloud",
        "https://data.cstcloud.cn/dav/",
        "https://data.cstcloud.cn/",
    ),
    (
        "InfiniCLOUD",
        "",
        "https://infini-cloud.net/en/support_service_webdavurl.html",
    ),
    (
        "Koofr",
        "https://app.koofr.net/dav/Koofr/",
        "https://app.koofr.net/",
    ),
    (
        "HiDrive",
        "https://webdav.hidrive.strato.com/",
        "https://www.strato.de/",
    ),
    (
        "Yandex Disk",
        "https://webdav.yandex.com/",
        "https://id.yandex.com/security/app-passwords",
    ),
    ("自定义", "", "https://help.jianguoyun.com/?tag=webdav"),
];
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub enabled: bool,
    pub provider: String,
    pub url: String,
    pub username: String,
    pub device_id: String,
    pub device_name: String,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: false,
            provider: "坚果云".into(),
            url: PROVIDERS[0].1.into(),
            username: String::new(),
            device_id: uuid::Uuid::new_v4().simple().to_string(),
            device_name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows".into()),
        }
    }
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 || !valid_id(&self.device_id) {
            return Err("云同步配置版本或设备身份无效".into());
        }
        let url = url::Url::parse(self.url.trim()).map_err(|_| "请输入有效的 WebDAV 地址")?;
        if !matches!(url.scheme(), "https" | "http")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("WebDAV 地址不能包含凭据、查询参数或片段".into());
        }
        if self.username.trim().is_empty() {
            return Err("请输入用户名".into());
        }
        if self.device_name.trim().is_empty() || self.device_name.len() > 256 {
            return Err("请输入有效的设备名称".into());
        }
        Ok(())
    }
    pub fn account(&self) -> String {
        crate::hash(
            format!(
                "{}\n{}",
                self.url.trim().trim_end_matches('/'),
                self.username.trim()
            )
            .as_bytes(),
        )
    }
    pub fn load_at(root: &Path) -> Result<Self> {
        let path = root.join("connection.json");
        match std::fs::read(path) {
            Ok(b) => serde_json::from_slice(&b).map_err(|_| "云同步配置损坏".into()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(_) => Err("无法读取云同步配置".into()),
        }
    }
    pub fn save_at(&self, root: &Path) -> Result<()> {
        if self.enabled {
            self.validate()?
        };
        write_json(&root.join("connection.json"), self)
    }
}
pub fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
pub fn root() -> Result<PathBuf> {
    Ok(
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("无法找到本地数据目录")?)
            .join("retype/sync"),
    )
}
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_bytes(
        path,
        &serde_json::to_vec_pretty(value).map_err(|_| "无法序列化同步数据")?,
    )
}
pub fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or("同步文件目录无效")?;
    std::fs::create_dir_all(parent).map_err(|_| "无法创建同步目录")?;
    let temp = parent.join(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    std::fs::write(&temp, bytes).map_err(|_| "无法写入同步文件")?;
    #[cfg(windows)]
    let result = retype_ai::secrets::replace_file(&temp, path);
    #[cfg(not(windows))]
    let result = std::fs::rename(&temp, path);
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
        return Err("无法替换同步文件".into());
    }
    Ok(())
}
pub fn trim_cache(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name
            .strip_suffix(".json")
            .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            || !entry.file_type().is_ok_and(|t| t.is_file())
        {
            continue;
        }
        if let Ok(metadata) = entry.metadata() {
            files.push((metadata.modified().ok(), metadata.len(), entry.path()));
        }
    }
    files.sort_by_key(|file| std::cmp::Reverse(file.0));
    let mut kept = 0u64;
    for (_, bytes, path) in files {
        kept = kept.saturating_add(bytes);
        if kept > 512 * 1024 * 1024 {
            let _ = std::fs::remove_file(path);
        }
    }
}
