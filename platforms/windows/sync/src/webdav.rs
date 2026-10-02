use crate::{
    config::{self, Config},
    Result,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

pub const MAX_OBJECT: usize = 32 * 1024 * 1024;
#[cfg(test)]
#[path = "webdav_tests.rs"]
mod tests;
pub struct WebDav {
    agent: ureq::Agent,
    root: Url,
    authorization: String,
    cstcloud: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    pub device: String,
    pub hash: String,
    pub bytes: usize,
    #[serde(default)]
    pub previous: Vec<String>,
}
impl WebDav {
    pub fn new(config: &Config, password: &str) -> Result<Self> {
        config.validate()?;
        if password.is_empty() {
            return Err("请输入云盘应用密码".into());
        }
        let mut base = Url::parse(config.url.trim()).map_err(|_| "WebDAV 地址无效")?;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let root = base.join("retype/v1/").map_err(|_| "WebDAV 地址无效")?;
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .http_status_as_error(false)
            .allow_non_standard_methods(true)
            .build()
            .into();
        Ok(Self {
            agent,
            root,
            authorization: format!(
                "Basic {}",
                STANDARD.encode(format!("{}:{password}", config.username.trim()))
            ),
            cstcloud: base.host_str() == Some("data.cstcloud.cn"),
        })
    }
    fn request(
        &self,
        method: &str,
        path: &str,
        bytes: &[u8],
        headers: &[(&str, &str)],
    ) -> Result<(u16, Vec<u8>)> {
        let url = self.root.join(path).map_err(|_| "云盘路径无效")?;
        if url.origin() != self.root.origin() {
            return Err("云盘请求地址超出配置范围".into());
        }
        let mut request = ureq::http::Request::builder()
            .method(method)
            .uri(url.as_str())
            .header("Authorization", &self.authorization)
            .header(
                "User-Agent",
                if self.cstcloud {
                    "retype Zotero/7.0"
                } else {
                    concat!("retype/", env!("CARGO_PKG_VERSION"))
                },
            );
        for (k, v) in headers {
            request = request.header(*k, *v);
        }
        let request = request.body(bytes).map_err(|_| "无法构造云盘请求")?;
        let mut response = self
            .agent
            .run(request)
            .map_err(|_| "云盘请求失败，请检查网络连接")?;
        let code = response.status().as_u16();
        if matches!(code, 401 | 403) {
            return Err("云盘鉴权失败，请检查用户名、应用密码和目录权限".into());
        }
        if matches!(code, 301 | 302 | 303 | 307 | 308) {
            return Err("云盘地址发生重定向，请填写最终 WebDAV 地址".into());
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_OBJECT as u64)
            .read_to_vec()
            .map_err(|_| "云盘响应过大或读取失败")?;
        Ok((code, body))
    }
    pub fn ensure_layout(&self) -> Result<()> {
        for path in ["../", "", "devices/", "objects/", "tmp/"] {
            let (status, _) = self.request("MKCOL", path, &[], &[])?;
            if !matches!(status, 200 | 201 | 204 | 405) {
                return Err(format!("无法创建云同步目录（HTTP {status}）"));
            }
        }
        Ok(())
    }
    pub fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let (status, bytes) = self.request("GET", path, &[], &[])?;
        match status {
            200 => Ok(Some(bytes)),
            404 => Ok(None),
            _ => Err(format!("下载同步数据失败（HTTP {status}）")),
        }
    }
    pub fn put(&self, path: &str, bytes: &[u8], immutable: bool) -> Result<()> {
        let mut headers = vec![("Content-Type", "application/json")];
        if immutable {
            headers.push(("If-None-Match", "*"));
        }
        let (status, _) = self.request("PUT", path, bytes, &headers)?;
        if immutable && status == 412 {
            if self.get(path)?.is_some_and(|existing| existing == bytes) {
                return Ok(());
            }
            return Err("云端同名版本的校验值不一致".into());
        }
        if !matches!(status, 200 | 201 | 204) {
            return Err(format!("上传同步数据失败（HTTP {status}）"));
        }
        Ok(())
    }
    pub fn devices(&self) -> Result<Vec<String>> {
        let (status,bytes)=self.request("PROPFIND","devices/",br#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/></d:prop></d:propfind>"#,&[("Depth","1"),("Content-Type","application/xml")])?;
        if status != 207 {
            return Err(format!("无法列举云端设备（HTTP {status}）"));
        }
        let mut href = false;
        let mut text = String::new();
        let mut names = Vec::new();
        for event in xml::reader::EventReader::new(bytes.as_slice()) {
            use xml::reader::XmlEvent;
            match event.map_err(|_| "云端目录响应格式无效")? {
                XmlEvent::StartElement { name, .. } if name.local_name == "href" => {
                    href = true;
                    text.clear();
                }
                XmlEvent::Characters(value) if href => text.push_str(&value),
                XmlEvent::EndElement { name } if name.local_name == "href" => {
                    href = false;
                    let target = self
                        .root
                        .join(text.trim())
                        .map_err(|_| "云端文件地址无效")?;
                    let devices = self.root.join("devices/").map_err(|_| "云端目录无效")?;
                    if target.origin() == devices.origin() {
                        if let Some(file) = target.path().strip_prefix(devices.path()) {
                            if let Some(id) =
                                file.strip_suffix(".json").filter(|id| config::valid_id(id))
                            {
                                names.push(id.into());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        names.sort();
        names.dedup();
        if names.len() > 128 {
            return Err("云端设备数量超过限制".into());
        }
        Ok(names)
    }
    pub fn test(&self) -> Result<()> {
        self.ensure_layout()?;
        let path = format!("tmp/{}.json", uuid::Uuid::new_v4().simple());
        let bytes = br#"{"retype_connection_test":1}"#;
        self.put(&path, bytes, false)?;
        let verified = self.get(&path)?.is_some_and(|b| b == bytes);
        let (code, _) = self.request("DELETE", &path, &[], &[])?;
        if !verified {
            return Err("云盘写入后校验失败".into());
        }
        if !matches!(code, 200 | 204 | 404) {
            return Err("云盘读写成功，但无法删除连接测试文件".into());
        }
        let _ = self.devices()?;
        Ok(())
    }
    pub fn publish<T: Serialize>(&self, device: &str, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|_| "无法编码同步数据")?;
        if bytes.len() > MAX_OBJECT {
            return Err("同步数据超过单个版本的大小限制".into());
        }
        let hash = crate::hash(&bytes);
        let previous_index = self
            .get(&format!("devices/{device}.json"))?
            .and_then(|b| serde_json::from_slice::<Index>(&b).ok())
            .filter(|i| {
                i.version == 1
                    && i.device == device
                    && i.hash.len() == 64
                    && i.hash.bytes().all(|b| b.is_ascii_hexdigit())
            });
        if previous_index
            .as_ref()
            .is_some_and(|index| index.hash == hash)
        {
            return Ok(());
        }
        let object = format!("objects/{hash}.json");
        self.put(&object, &bytes, true)?;
        if self.get(&object)?.is_none_or(|b| crate::hash(&b) != hash) {
            return Err("云端版本上传后校验失败".into());
        }
        let mut previous = previous_index.map_or_else(Vec::new, |index| {
            let mut history = vec![index.hash];
            history.extend(index.previous);
            history
        });
        previous.retain(|old| {
            old.len() == 64 && old.bytes().all(|b| b.is_ascii_hexdigit()) && old != &hash
        });
        let expired = if previous.len() > 3 {
            previous.split_off(3)
        } else {
            vec![]
        };
        self.put(
            &format!("devices/{device}.json"),
            &serde_json::to_vec(&Index {
                version: 1,
                device: device.into(),
                hash,
                bytes: bytes.len(),
                previous,
            })
            .map_err(|_| "无法编码版本索引")?,
            false,
        )?;
        // Retain three previous complete versions. An object is deleted only
        // after verifying it belongs to this device, never by an arbitrary href.
        for old in expired {
            let path = format!("objects/{old}.json");
            if let Ok(Some(bytes)) = self.get(&path) {
                if serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .is_some_and(|v| v["device"].as_str() == Some(device))
                {
                    let _ = self.request("DELETE", &path, &[], &[]);
                }
            }
        }
        Ok(())
    }
    pub fn download<T: serde::de::DeserializeOwned>(&self, device: &str) -> Result<T> {
        self.download_inner(device, None)
    }
    pub fn download_cached<T: serde::de::DeserializeOwned>(
        &self,
        device: &str,
        cache: &std::path::Path,
    ) -> Result<T> {
        let result = self.download_inner(device, Some(cache));
        config::trim_cache(cache);
        result
    }
    fn download_inner<T: serde::de::DeserializeOwned>(
        &self,
        device: &str,
        cache: Option<&std::path::Path>,
    ) -> Result<T> {
        let index: Index = serde_json::from_slice(
            &self
                .get(&format!("devices/{device}.json"))?
                .ok_or("云端设备索引缺失")?,
        )
        .map_err(|_| "云端版本索引格式无效")?;
        if index.version != 1
            || index.device != device
            || index.hash.len() != 64
            || !index.hash.bytes().all(|b| b.is_ascii_hexdigit())
            || index.bytes > MAX_OBJECT
        {
            return Err("云端版本索引无效或不兼容".into());
        }
        let cached = cache
            .map(|root| root.join(format!("{}.json", index.hash)))
            .and_then(|path| std::fs::read(path).ok())
            .filter(|bytes| bytes.len() == index.bytes && crate::hash(bytes) == index.hash);
        let bytes = if let Some(bytes) = cached {
            bytes
        } else {
            let bytes = self
                .get(&format!("objects/{}.json", index.hash))?
                .ok_or("云端版本文件缺失")?;
            if bytes.len() != index.bytes || crate::hash(&bytes) != index.hash {
                return Err("云端版本校验失败，本地数据未应用".into());
            }
            if let Some(root) = cache {
                config::write_bytes(&root.join(format!("{}.json", index.hash)), &bytes)?;
            }
            bytes
        };
        if bytes.len() != index.bytes || crate::hash(&bytes) != index.hash {
            return Err("云端版本校验失败，本地数据未应用".into());
        }
        serde_json::from_slice(&bytes).map_err(|_| "云端数据格式无效".into())
    }
}
