//! A3 blocking work. Called on Android IO workers, never the IME key dispatcher.
use retype_ai::{
    config::{Config as AiConfig, Provider},
    service,
};
use retype_sync::{
    config::{self, Config},
    model::{Snapshot, Stamp, State},
    statistics::{self, Bucket},
    webdav::WebDav,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{BufReader, Cursor},
    path::Path,
    sync::Arc,
};
type Result<T> = std::result::Result<T, String>;
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().ok_or_else(|| format!("Missing {key}"))
}
pub fn perform(v: Value) -> Result<Value> {
    match field(&v, "type")? {
        "models" => {
            let p: Provider =
                serde_json::from_value(v["provider"].clone()).map_err(|e| e.to_string())?;
            Ok(json!(service::models_with_key(&p, field(&v, "key")?)?))
        }
        "translate" => {
            let cfg: AiConfig =
                serde_json::from_value(v["config"].clone()).map_err(|e| e.to_string())?;
            let p = cfg.selected()?;
            Ok(json!(service::translate_with_key(
                p,
                &cfg.model,
                field(&v, "text")?,
                &cfg.target,
                &cfg.instructions,
                &cfg.reasoning,
                cfg.timeout_seconds,
                field(&v, "key")?
            )?))
        }
        "compilePack" => {
            let source = std::fs::File::open(field(&v, "source")?).map_err(|e| e.to_string())?;
            let tsv = retype_dict::rime::tsv(BufReader::new(source))?;
            let destination = Path::new(field(&v, "destination")?);
            let file = std::fs::File::create(destination).map_err(|e| e.to_string())?;
            let count =
                retype_dict::binary::compile(Cursor::new(tsv), &file).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            Ok(json!({"words":count}))
        }
        "syncTest" => {
            let cfg: Config =
                serde_json::from_value(v["config"].clone()).map_err(|e| e.to_string())?;
            WebDav::new(&cfg, field(&v, "password")?)?.test()?;
            Ok(json!({"message":"连接成功"}))
        }
        "sync" => sync(v),
        _ => Err("Unknown mobile operation".into()),
    }
}
fn read<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).map_err(|_| "本地同步记录损坏".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.to_string()),
    }
}
fn validate(s: &Snapshot, id: &str) -> Result<()> {
    if s.version != 1
        || s.device != id
        || !config::valid_id(id)
        || s.name.len() > 256
        || s.fields.len() > 2000
        || !s.learning.valid()
    {
        return Err("云端数据无效或版本不兼容".into());
    }
    let mut buckets = Vec::new();
    statistics::merge(&mut buckets, &s.statistics)?;
    for (key, f) in &s.fields {
        if key.len() > 256
            || f.ancestors.len() > 20000
            || f.stamp.revision == u64::MAX
            || !config::valid_id(&f.stamp.device)
            || f.ancestors.iter().any(|s| !config::valid_id(&s.device))
        {
            return Err("云端设置版本无效".into());
        }
        if key.starts_with("providers/") && !f.value.is_null() {
            let p: Provider =
                serde_json::from_value(f.value.clone()).map_err(|_| "云端提供商无效")?;
            if format!("providers/{}", p.id) != *key
                || p.base_url.contains(['@', '?', '#'])
                || p.models.len() > 10000
            {
                return Err("云端提供商无效".into());
            }
        }
    }
    Ok(())
}
fn sync(v: Value) -> Result<Value> {
    let cfg: Config = serde_json::from_value(v["config"].clone()).map_err(|e| e.to_string())?;
    cfg.validate()?;
    let cloud = WebDav::new(&cfg, field(&v, "password")?)?;
    let root = Path::new(field(&v, "root")?).join(cfg.account());
    let state_file = root.join("state.json");
    let mut state: State = read(&state_file)?;
    let current: BTreeMap<String, Value> =
        serde_json::from_value(v["values"].clone()).map_err(|e| e.to_string())?;
    if state.initialized {
        let mut current = current.clone();
        for k in state
            .fields
            .keys()
            .filter(|k| k.starts_with("providers/") && !current.contains_key(*k))
            .cloned()
            .collect::<Vec<_>>()
        {
            current.insert(k, Value::Null);
        }
        state.capture(&cfg.device_id, &current);
        config::write_json(&state_file, &state)?;
    }
    if let Some(key) = v["resolveKey"].as_str() {
        let stamp: Stamp =
            serde_json::from_value(v["resolveStamp"].clone()).map_err(|e| e.to_string())?;
        if !state.resolve(
            &cfg.device_id,
            key,
            v["useRemote"].as_bool().unwrap_or(false),
            Some(&stamp),
        ) {
            return Err("冲突已经变化，请重新同步".into());
        }
    }
    cloud.ensure_layout()?;
    let mut remote = Vec::new();
    for id in cloud.devices()? {
        let s: Snapshot = cloud.download_cached(&id, &root.join("cache"))?;
        validate(&s, &id)?;
        remote.push(s);
    }
    if !state.initialized && !remote.is_empty() && v["confirm"].as_bool() != Some(true) {
        return Ok(
            json!({"awaitingMerge":true,"message":"发现已有云端数据，请确认首次合并","conflicts":[]}),
        );
    }
    for s in &remote {
        state.merge(s);
    }
    if !state.initialized {
        let missing = current
            .into_iter()
            .filter(|(k, _)| !state.fields.contains_key(k))
            .collect();
        state.capture(&cfg.device_id, &missing);
        state.initialized = true;
    }
    let local: Vec<Bucket> =
        serde_json::from_value(v["statistics"].clone()).map_err(|e| e.to_string())?;
    let mut imported: Vec<Bucket> = read(&root.join("statistics.json"))?;
    statistics::merge(&mut imported, &local)?;
    for s in &remote {
        statistics::merge(&mut imported, &s.statistics)?;
    }
    let learning = {
        let _lock = super::LEARNING_IO.lock().unwrap_or_else(|p| p.into_inner());
        let system: Arc<dyn retype_pinyin::Lexicon> =
            retype_dict::binary::open_shared(Path::new(field(&v, "dictionary")?))
                .map_err(|e| e.to_string())?;
        let mut store =
            retype_learning_store::store::Store::open(Path::new(field(&v, "database")?), system)
                .map_err(|e| e.to_string())?;
        for s in &remote {
            store.sync_merge(&s.learning).map_err(|e| e.to_string())?;
        }
        store.sync_export().map_err(|e| e.to_string())?
    };
    let snapshot = Snapshot {
        version: 1,
        device: cfg.device_id.clone(),
        name: cfg.device_name.clone(),
        fields: state.fields.clone(),
        learning,
        statistics: imported.clone(),
    };
    // Preserve pending state across interrupted publications; immutable uploads are retryable.
    config::write_json(&state_file, &state)?;
    cloud.publish(&cfg.device_id, &snapshot)?;
    config::write_json(&root.join("statistics.json"), &imported)?;
    Ok(
        json!({"awaitingMerge":false,"values":state.values(),"statistics":imported,"conflicts":state.conflicts,"message":if state.conflicts.is_empty(){"同步完成"}else{"数据已同步，有设置冲突需要处理"}}),
    )
}
