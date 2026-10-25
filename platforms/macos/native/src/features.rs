use retype_ai::config::{Config as AiConfig, Provider};
use retype_dict::{Layer, LayeredDict};
use retype_sync::statistics::Bucket;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    path::Path,
    sync::Arc,
    time::Duration,
};
type Result<T> = std::result::Result<T, String>;
#[cfg(test)]
#[path = "feature_tests.rs"]
mod tests;
pub use retype_resources::packs::Pack;
pub fn packs() -> Result<Vec<Pack>> {
    retype_resources::packs::catalog()
}
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().ok_or_else(|| format!("Missing {key}"))
}
pub fn dictionary(root: &Path, mask: u32) -> Result<Arc<dyn retype_pinyin::Lexicon>> {
    let status = retype_resources::activate(&root.join("dictionaries"))?;
    let base = retype_dict::binary::open_shared(&status.base.ok_or("基础词库尚未安装")?)
        .map_err(|e| e.to_string())?;
    let total = base.total_frequency();
    let mut dict = LayeredDict::new();
    dict.push(Layer {
        name: "base",
        dict: base,
        boost: 0.,
    });
    for (i, p) in packs()?.iter().enumerate() {
        if mask & (1 << i) != 0 {
            let path = root.join("dict-packs").join(format!("{}.bin", p.id));
            if let Ok(pack) = retype_dict::binary::open_shared(&path) {
                let boost = (pack.total_frequency() / total.max(1.)).ln() as f32;
                dict.push(Layer {
                    name: "optional",
                    dict: pack,
                    boost,
                });
            }
        }
    }
    Ok(Arc::new(dict))
}
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .timeout_connect(Some(Duration::from_secs(20)))
        .max_redirects(6)
        .build()
        .into()
}
fn read_url(url: &str, limit: u64, asset: bool) -> Result<Vec<u8>> {
    let mut response = agent()
        .get(url)
        .header("User-Agent", "retype-macos")
        .header(
            "Accept",
            if asset {
                "application/octet-stream"
            } else {
                "application/vnd.github+json"
            },
        )
        .header("X-GitHub-Api-Version", "2026-03-10")
        .header("Accept-Encoding", "identity")
        .call()
        .map_err(|_| "网络请求失败，请检查网络后重试")?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "读取响应失败")?;
    if bytes.len() as u64 > limit {
        return Err("下载内容超过大小限制".into());
    }
    Ok(bytes)
}
fn install_pack(root: &Path, id: &str) -> Result<Value> {
    static INSTALL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = INSTALL.lock().unwrap_or_else(|p| p.into_inner());
    let pack = packs()?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or("无效词库")?;
    let dir = root.join("dict-packs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.bin", pack.id));
    let source =
        std::fs::read_to_string(dir.join(format!("{}.source", pack.id))).unwrap_or_default();
    let expected =
        std::fs::read_to_string(dir.join(format!("{}.sha256", pack.id))).unwrap_or_default();
    if source == pack.sha256
        && expected.len() == 64
        && std::fs::File::open(&path).ok().is_some_and(|f| {
            retype_updater::sha256_hex_reader(f).is_ok_and(|actual| actual == expected)
        })
        && retype_dict::binary::open_shared(&path).is_ok()
    {
        return Ok(json!({"installed":true}));
    }
    let raw = read_url(&pack.source_url(), pack.bytes, false)?;
    let staging = path.with_extension(format!("{}.download", retype_ai::config::new_id()));
    retype_resources::packs::compile_bytes(&pack, &raw, &staging)?;
    let mut permissions = std::fs::metadata(&staging)
        .map_err(|e| e.to_string())?
        .permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&staging, permissions).map_err(|e| e.to_string())?;
    retype_dict::binary::open_shared(&staging).map_err(|e| e.to_string())?;
    let checksum = retype_updater::sha256_hex_reader(
        std::fs::File::open(&staging).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if path.exists() {
        std::fs::rename(
            &path,
            path.with_extension(format!("{}.obsolete", retype_ai::config::new_id())),
        )
        .map_err(|e| e.to_string())?;
    }
    std::fs::rename(&staging, &path).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}.source", pack.id)), pack.sha256)
        .map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}.sha256", pack.id)), checksum).map_err(|e| e.to_string())?;
    Ok(json!({"installed":true}))
}
pub fn sync_values(p: &Value) -> Result<Value> {
    let ai: AiConfig = serde_json::from_value(p["ai"].clone()).map_err(|_| "AI配置无效")?;
    let mut values = json!({"input.scheme":p["input"]["scheme"],"input.english":p["input"]["english"],"input.english_spelling":p["input"]["spelling"],"input.symbol_completion":p["input"]["symbols"],"dictionary.enabled":p["packs"],"updates.auto_check":p["autoCheck"],"voice.settings":ai.voice,"translation.model":{"provider":ai.provider,"model":ai.model},"translation.target":ai.target,"translation.preview":ai.preview,"translation.instructions":ai.instructions,"translation.reasoning":ai.reasoning,"translation.timeout":ai.timeout_seconds});
    for provider in ai.providers {
        values[format!("providers/{}", provider.id)] =
            serde_json::to_value(&provider).map_err(|e| e.to_string())?;
    }
    // Physical keycodes differ from Windows VK values; retain a separate platform field.
    values["shortcuts.macos"] = p["shortcuts"].clone();
    values["shortcuts"] = p.get("portableShortcuts").cloned().unwrap_or(
        serde_json::to_value(retype_ai::config::Shortcuts::default()).map_err(|e| e.to_string())?,
    );
    if let Some(voice) = values["voice.settings"].as_object_mut() {
        voice.remove("microphone");
    }
    Ok(values)
}
pub fn apply_values(current: &Value, v: &Value) -> Result<Value> {
    let mut out = current.clone();
    let scheme = v["input.scheme"]
        .as_u64()
        .filter(|n| *n <= 1)
        .ok_or("云端拼音方案无效")?;
    let mask = v["dictionary.enabled"]
        .as_u64()
        .filter(|n| *n <= 127)
        .ok_or("云端词库设置无效")?;
    out["input"]["scheme"] = json!(scheme);
    out["packs"] = json!(mask);
    for (local, remote) in [
        ("english", "input.english"),
        ("spelling", "input.english_spelling"),
        ("symbols", "input.symbol_completion"),
    ] {
        out["input"][local] = json!(v[remote].as_bool().ok_or("云端输入设置无效")?);
    }
    let mut ai: AiConfig =
        serde_json::from_value(out["ai"].clone()).map_err(|_| "本地AI配置无效")?;
    ai.providers = v
        .as_object()
        .ok_or("云端设置无效")?
        .iter()
        .filter(|(k, value)| k.starts_with("providers/") && !value.is_null())
        .map(|(k, value)| {
            let p: Provider =
                serde_json::from_value(value.clone()).map_err(|_| "云端提供商无效")?;
            if format!("providers/{}", p.id) != *k {
                return Err("云端提供商身份不一致");
            }
            Ok(p)
        })
        .collect::<std::result::Result<_, _>>()?;
    ai.provider = field(&v["translation.model"], "provider")?.into();
    ai.model = field(&v["translation.model"], "model")?.into();
    ai.target = field(v, "translation.target")?.into();
    ai.instructions = field(v, "translation.instructions")?.into();
    ai.reasoning = field(v, "translation.reasoning")?.into();
    if !["none", "default", "minimal", "low", "medium", "high"].contains(&ai.reasoning.as_str()) {
        return Err("云端思考等级无效".into());
    }
    ai.preview = v["translation.preview"]
        .as_bool()
        .ok_or("云端翻译设置无效")?;
    ai.timeout_seconds = v["translation.timeout"]
        .as_u64()
        .filter(|n| (5..=180).contains(n))
        .ok_or("云端翻译超时无效")?;
    if let Some(voice) = v.get("voice.settings") {
        ai.voice = serde_json::from_value(voice.clone()).map_err(|_| "云端语音设置无效")?;
        ai.voice.validate()?;
    }
    out["ai"] = serde_json::to_value(ai).map_err(|e| e.to_string())?;
    out["autoCheck"] = json!(v["updates.auto_check"].as_bool().unwrap_or(true));
    if let Some(keys) = v.get("shortcuts.macos") {
        validate_shortcuts(keys)?;
        out["shortcuts"] = keys.clone();
    }
    if let Some(keys) = v.get("shortcuts") {
        out["portableShortcuts"] = keys.clone();
    }
    Ok(out)
}
pub fn validate_shortcuts(v: &Value) -> Result<()> {
    let mut used = Vec::new();
    for name in ["mode", "translate", "voice"] {
        let b = &v[name];
        let key = b["key"].as_u64().ok_or("快捷键无效")?;
        let mods = b["mods"]
            .as_u64()
            .filter(|m| *m <= 15)
            .ok_or("快捷键修饰键无效")?;
        let tap = b["tap"].as_bool().unwrap_or(false);
        if mods.count_ones() > 2 {
            return Err("快捷键最多使用两个修饰键".into());
        }
        if key > 127
            || (tap && !matches!(key, 56 | 60 | 59 | 62 | 58 | 61))
            || (!tap && (mods == 0 || mods == 4))
        {
            return Err("请选择修饰键组合，或单独的 Shift／Control／Option".into());
        }
        if !tap && mods & 8 != 0 && matches!(key, 49 | 12 | 13 | 48) {
            return Err("此组合用于系统或窗口操作".into());
        }
        if used.contains(&(key, mods, tap)) {
            return Err("快捷键不能重复".into());
        }
        used.push((key, mods, tap));
    }
    Ok(())
}
pub fn perform(v: Value) -> Result<Value> {
    match field(&v,"type")? {
        "providerPresets"=>Ok(json!(retype_ai::config::PRESETS.iter().map(|(name,url,kind)|json!({"name":name,"url":url,"kind":kind})).collect::<Vec<_>>())),
        "cloudAccount"=>{let config:retype_sync::config::Config=serde_json::from_value(v["config"].clone()).map_err(|_|"云盘配置无效")?;Ok(json!(config.account()))},
        "defaults"=>{
            let cloud=retype_sync::config::Config{device_name:"Mac".into(),..Default::default()};
            Ok(json!({"input":{"scheme":0,"english":true,"spelling":true,"symbols":true},"packs":0,"autoCheck":true,"ai":AiConfig::default(),"cloud":cloud,"shortcuts":{"mode":{"key":56,"mods":0,"tap":true},"translate":{"key":29,"mods":3,"tap":false},"voice":{"key":61,"mods":0,"tap":true}}}))
        }
        #[cfg(target_os="macos")]
        "lanStatus"=>{let root=retype_sync::lan::root()?;Ok(json!({"config":retype_sync::lan::load_at(&root)?,"status":retype_sync::lan::status_at(&root)}))},
        #[cfg(target_os="macos")]
        "lanCommand"=>{let command=serde_json::from_value(v["command"].clone()).map_err(|_|"设备操作无效")?;retype_sync::lan::command(command)?;Ok(json!(true))},
        #[cfg(target_os="macos")]
        "lanResume"=>{let root=retype_sync::lan::root()?;if retype_sync::lan::load_at(&root)?.enabled{retype_sync::lan::command(retype_sync::lan::Command::Enable(true))?;}Ok(json!(true))},
        "packCatalog"=>Ok(Value::Array(packs()?.into_iter().map(|p|json!({"id":p.id,"title":p.title,"description":p.description,"bytes":p.bytes})).collect())),
        "installPack"=>install_pack(Path::new(field(&v,"root")?),field(&v,"id")?),
        "validateShortcuts"=>{validate_shortcuts(&v["shortcuts"])?;Ok(json!(true))},
        "statistics"=>{let rows=super::statistics::export(Path::new(field(&v,"root")?),field(&v,"device")?)?;super::statistics::summary(&rows,v["period"].as_u64().unwrap_or(0).min(3) as u32,v["version"].as_str())},
        "syncValues"=>sync_values(&v["preferences"]),
        "applyValues"=>apply_values(&v["preferences"],&v["values"]),
        "cloudSync"=>{
            let root=Path::new(field(&v,"root")?);let p=&v["preferences"];
            let mut request=v.clone();request["type"]=json!("sync");request["config"]=p["cloud"].clone();request["values"]=sync_values(p)?;
            request["root"]=json!(root.join("sync"));request["database"]=json!(root.join("learning.db"));
            request["dictionary"]=json!(retype_resources::status(&root.join("dictionaries"))?.base.map(|p|p.to_string_lossy().into_owned()).unwrap_or_default());
            request["statistics"]=json!(super::statistics::export(root,field(&p["cloud"],"device_id")?)?);
            let result=retype_services::perform(request)?;
            // The UI applies settings/statistics together only if its revision is unchanged.
            Ok(result)
        }
        "mergeStatistics"=>{
            let root=Path::new(field(&v,"root")?);let rows:Vec<Bucket>=serde_json::from_value(v["statistics"].clone()).map_err(|_|"云端统计无效")?;
            let mut validated=Vec::new();retype_sync::statistics::merge(&mut validated,&rows)?;
            retype_sync::config::write_json(&root.join("statistics-imported.json"),&validated)?;Ok(json!(true))
        }
        "checkUpdate"=>check_update(),
        "downloadUpdate"=>download_update(Path::new(field(&v,"root")?),&v["release"]),
        _=>retype_services::perform(v),
    }
}
fn check_update() -> Result<Value> {
    let bytes = read_url(
        "https://api.github.com/repos/L-Chris/retype/releases?per_page=30",
        4 * 1024 * 1024,
        false,
    )?;
    parse_updates(&serde_json::from_slice(&bytes).map_err(|_| "更新信息无效")?)
}
fn parse_updates(releases: &Value) -> Result<Value> {
    let current =
        retype_updater::parse_version(env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())?;
    retype_updater::package::select(
        releases,
        &[retype_updater::Platform::MacosUniversal],
        Some(&current),
    )
    .map_err(|e| e.to_string())?
    .map(|p| serde_json::to_value(p).map_err(|e| e.to_string()))
    .unwrap_or_else(|| Ok(json!({"current":true})))
}
fn download_update(root: &Path, release: &Value) -> Result<Value> {
    let url = field(release, "url")?;
    let hash_url = field(release, "hashUrl")?;
    if !retype_updater::package::valid_asset_url(url)
        || !retype_updater::package::valid_asset_url(hash_url)
        || url == hash_url
    {
        return Err("更新附件地址无效，请重新检查更新".into());
    }
    let hash_bytes = read_url(hash_url, 4096, true)?;
    let sums = std::str::from_utf8(&hash_bytes).map_err(|_| "更新校验文件无效")?;
    let size = release["size"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 256 * 1024 * 1024)
        .ok_or("更新大小无效")?;
    let version = field(release, "version")?;
    retype_updater::parse_version(version).map_err(|e| e.to_string())?;
    let name = format!("retype-{version}-macos-universal.zip");
    let expected = retype_updater::expected_for(sums, &name).ok_or("更新校验文件中没有对应附件")?;
    let dir = root.join("updates");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&name);
    let temp = path.with_extension("part");
    let mut response = agent()
        .get(url)
        .header("User-Agent", "retype-macos")
        .header("Accept", "application/octet-stream")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .call()
        .map_err(|_| "更新下载失败")?;
    let result = (|| {
        let mut file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
        let n = std::io::copy(
            &mut response.body_mut().as_reader().take(size + 1),
            &mut file,
        )
        .map_err(|_| "读取更新失败")?;
        file.flush().map_err(|e| e.to_string())?;
        drop(file);
        if n != size {
            return Err("更新文件不完整".into());
        }
        let mut file = std::fs::File::open(&temp).map_err(|e| e.to_string())?;
        retype_updater::verify_sha256_reader(&mut file, &expected).map_err(|e| e.to_string())?;
        std::fs::rename(&temp, &path).map_err(|e| e.to_string())?;
        Ok(json!({"path":path,"version":version}))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
