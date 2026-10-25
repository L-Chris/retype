//! Shared desktop/mobile feature operations. Call blocking operations on workers.
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
use std::{collections::BTreeMap, path::Path, sync::Arc};
pub static LEARNING_IO: std::sync::Mutex<()> = std::sync::Mutex::new(());

type Result<T> = std::result::Result<T, String>;
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().ok_or_else(|| format!("Missing {key}"))
}

pub fn perform(v: Value) -> Result<Value> {
    match field(&v, "type")? {
        "featurePolicy" => Ok(
            json!({"clipboard":{"maxText":retype_sync::clipboard_protocol::MAX_TEXT,"maxFrame":retype_sync::clipboard_protocol::MAX_FRAME,"ttl":retype_sync::clipboard_protocol::TTL_MS,"port":retype_sync::lan_address::DEFAULT_PORT},"statistics":{"lengthLabels":retype_types::statistics::LENGTH_LABELS,"lengthBuckets":retype_types::statistics::LENGTH_BUCKETS}}),
        ),
        "resourceCatalog" => {
            let catalog = retype_resources::Catalog::builtin()?;
            Ok(
                json!({"components":catalog.components.iter().map(|c|json!({"id":c.id,"url":c.url(),"bytes":c.bytes,"sha256":c.sha256,"compressedBytes":c.compressed_bytes,"compressedSha256":c.compressed_sha256})).collect::<Vec<_>>()}),
            )
        }
        "resourceStatus" | "resourcePrepare" | "resourceInstall" | "resourceImport" => {
            let root = Path::new(field(&v, "root")?);
            let cancelled = std::sync::atomic::AtomicBool::new(false);
            let state = match field(&v, "type")? {
                "resourcePrepare" => {
                    let paths: Vec<std::path::PathBuf> =
                        serde_json::from_value(v["legacy"].clone()).unwrap_or_default();
                    retype_resources::migrate(root, &paths)?
                }
                "resourceInstall" => retype_resources::install_archive(
                    root,
                    field(&v, "id")?,
                    Path::new(field(&v, "source")?),
                    &cancelled,
                )?,
                "resourceImport" => retype_resources::import_bundle(
                    root,
                    Path::new(field(&v, "source")?),
                    &cancelled,
                )?,
                _ => retype_resources::status(root)?,
            };
            if let Some(english) = &state.english {
                retype_english::install(english)?;
            }
            serde_json::to_value(state).map_err(|e| e.to_string())
        }
        "symbolRules" => {
            let key = field(&v, "key")?.chars().next().ok_or("Missing symbol")?;
            let rules = retype_types::symbols::rules(key, v["chinese"].as_bool().unwrap_or(true));
            let before = v["before"].as_str().and_then(|s| s.chars().last());
            let after = v["after"].as_str().and_then(|s| s.chars().next());
            let opening = rules.opening;
            let pair = opening.is_some_and(|(left, right)| {
                retype_types::symbols::eligible(left, right, before, after)
            });
            Ok(
                json!({"left":opening.map(|p|p.0.to_string()),"right":opening.map(|p|p.1.to_string()),"close":rules.closing.map(|c|c.to_string()),"pair":pair}),
            )
        }
        "voiceStart" | "voicePush" | "voiceStop" | "voicePoll" | "voiceCancel" => {
            use retype_ai::{voice::Settings, voice_service::Controller};
            use std::sync::{Mutex, OnceLock};
            static VOICE: OnceLock<Mutex<Controller>> = OnceLock::new();
            let mut controller = VOICE
                .get_or_init(Default::default)
                .lock()
                .map_err(|_| "语音状态异常")?;
            let id = field(&v, "id")?;
            match field(&v, "type")? {
                "voiceStart" => {
                    let cfg: AiConfig =
                        serde_json::from_value(v["config"].clone()).map_err(|_| "语音配置无效")?;
                    let settings: Settings = serde_json::from_value(v["settings"].clone())
                        .map_err(|_| "语音配置无效")?;
                    let provider = settings.selected(&cfg)?.clone();
                    controller.start(id.into(), provider, settings, field(&v, "key")?.into())?;
                }
                "voicePush" => {
                    use base64::{engine::general_purpose::STANDARD, Engine};
                    let bytes = STANDARD
                        .decode(field(&v, "pcm")?)
                        .map_err(|_| "音频数据无效")?;
                    if bytes.len() > 32000 || bytes.len() % 2 != 0 {
                        return Err("音频帧长度无效".into());
                    }
                    controller.get(id)?.push(
                        bytes
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|s| i16::from_le_bytes(*s))
                            .collect(),
                    )?;
                }
                "voiceStop" => controller.get(id)?.stop()?,
                "voiceCancel" => controller.get(id)?.cancel(),
                _ => {}
            }
            serde_json::to_value(controller.get(id)?.snapshot()).map_err(|_| "语音状态无效".into())
        }
        "voiceCounts" => {
            let (chinese, english) = retype_ai::voice::counts(field(&v, "text")?);
            Ok(json!({"chinese":chinese,"english":english}))
        }
        "countWords" => {
            let state: Vec<u8> = serde_json::from_value(v["state"].clone())
                .map_err(|_| "Invalid word counter state")?;
            let mut counter = retype_types::statistics::WordCounter::restore(&state)
                .ok_or("Invalid word counter state")?;
            let mut words = counter.feed(v["text"].as_str().unwrap_or(""));
            if v["backspace"].as_bool().unwrap_or(false) {
                counter.backspace();
            }
            if v["boundary"].as_bool().unwrap_or(false) {
                words = words.saturating_add(counter.finish());
            }
            Ok(json!({"words":words,"state":counter.checkpoint()}))
        }
        "updateAsset" => {
            Ok(json!({"valid":retype_updater::package::valid_asset_url(field(&v,"url")?)}))
        }
        "updateVersionCode" => Ok(
            json!({"code":retype_updater::package::android_version_code(field(&v,"version")?).map_err(|e|e.to_string())?}),
        ),
        "updatePackageNames" => {
            let abis: Vec<String> =
                serde_json::from_value(v["abis"].clone()).map_err(|_| "设备架构无效")?;
            Ok(
                json!({"names":retype_updater::package::android_targets(&abis).into_iter().map(|p|retype_updater::package::asset_name(v["version"].as_str().unwrap_or(""),p)).collect::<Vec<_>>()}),
            )
        }
        "updateSelect" => {
            let abis: Vec<String> =
                serde_json::from_value(v["abis"].clone()).map_err(|_| "设备架构无效")?;
            let selected = retype_updater::package::select(
                &v["releases"],
                &retype_updater::package::android_targets(&abis),
                None,
            )
            .map_err(|e| e.to_string())?;
            let release = selected.map(|p| {
                let code =
                    retype_updater::package::android_version_code(&p.version).unwrap_or_default();
                let mut value = serde_json::to_value(p).unwrap_or_default();
                value["code"] = json!(code);
                value
            });
            Ok(json!({"release":release}))
        }
        "lanEndpoint" => {
            use retype_sync::lan_address as address;
            let value = field(&v, "value")?;
            match field(&v, "operation")? {
                "normalize" => Ok(json!({"address":address::normalize_address(value)?})),
                "list" => Ok(json!({"addresses":address::normalize_list(value)?})),
                "priority" => Ok(json!({"priority":address::priority(value)?})),
                "allowed" | "overlay" => {
                    let ip = value.parse().map_err(|_| "设备 IP 无效")?;
                    Ok(json!({"allowed":address::allowed(ip),"overlay":address::is_tailscale(ip)}))
                }
                _ => Err("设备地址操作无效".into()),
            }
        }
        "clipboard" => {
            use retype_sync::clipboard_protocol::{Clip, Journal};
            use std::sync::{Mutex, OnceLock};
            static JOURNAL: OnceLock<Mutex<Journal>> = OnceLock::new();
            let mut journal = JOURNAL
                .get_or_init(Default::default)
                .lock()
                .map_err(|_| "剪贴板状态异常")?;
            let now = v["now"].as_u64().ok_or("剪贴板时间无效")?;
            match field(&v, "operation")? {
                "local" => Ok(
                    json!({"event":journal.local(field(&v,"origin")?,v["seq"].as_u64().ok_or("剪贴板序号无效")?,field(&v,"text")?.into(),now)}),
                ),
                "accept" => {
                    let clip: Clip =
                        serde_json::from_value(v["event"].clone()).map_err(|_| "剪贴板消息无效")?;
                    Ok(json!({"accepted":journal.accept(clip,now)}))
                }
                "clear" => {
                    journal.latest = None;
                    Ok(json!({}))
                }
                "valid" => {
                    let clip: Clip =
                        serde_json::from_value(v["event"].clone()).map_err(|_| "剪贴板消息无效")?;
                    Ok(
                        json!({"valid":clip.valid(now),"remaining":clip.created.saturating_add(retype_sync::clipboard_protocol::TTL_MS).saturating_sub(now)}),
                    )
                }
                _ => Err("剪贴板操作无效".into()),
            }
        }
        "pairBegin" | "pairFinish" | "pairCancel" => retype_sync::pairing::mobile(&v),
        "models" => {
            let p: Provider =
                serde_json::from_value(v["provider"].clone()).map_err(|e| e.to_string())?;
            Ok(json!(service::models_with_key(&p, field(&v, "key")?)?))
        }
        "testProvider" => {
            let p: Provider =
                serde_json::from_value(v["provider"].clone()).map_err(|_| "提供商配置无效")?;
            service::test_with_key(&p, field(&v, "model")?, field(&v, "key")?)?;
            Ok(json!("连接成功"))
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
        "packCatalog" => {
            let rows = retype_resources::packs::catalog()?
                .into_iter()
                .map(|p| {
                    let url = p.source_url();
                    let mut value = serde_json::to_value(p).unwrap_or_default();
                    value["url"] = json!(url);
                    value
                })
                .collect::<Vec<_>>();
            Ok(json!({"packs":rows}))
        }
        "compilePack" => {
            let count = retype_resources::packs::compile_source(
                field(&v, "id")?,
                Path::new(field(&v, "source")?),
                Path::new(field(&v, "destination")?),
            )?;
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
        state.capture_settings(&cfg.device_id, &current);
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
        retype_sync::schema::validate_snapshot(&s, &id)?;
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
            .iter()
            .filter(|(k, _)| !state.fields.contains_key(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
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
        let _lock = LEARNING_IO.lock().unwrap_or_else(|p| p.into_inner());
        let system: Arc<dyn retype_pinyin::Lexicon> =
            match v["dictionary"].as_str().filter(|p| !p.is_empty()) {
                Some(path) => {
                    retype_dict::binary::open_shared(Path::new(path)).map_err(|e| e.to_string())?
                }
                None => Arc::new(retype_dict::MemoryDict::default()),
            };
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
        json!({"awaitingMerge":false,"values":state.values_for(&current),"statistics":imported,"conflicts":state.conflicts,"message":if state.conflicts.is_empty(){"同步完成"}else{"数据已同步，有设置冲突需要处理"}}),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod symbol_tests {
    use super::*;
    #[test]
    fn native_rules_preserve_mappings_even_when_pairing_is_ineligible() {
        let result = perform(
            json!({"type":"symbolRules", "key":"\"", "chinese":true, "before":"hao", "after":"”"}),
        )
        .unwrap();
        assert_eq!(result["left"], "“");
        assert_eq!(result["close"], "”");
        assert_eq!(result["pair"], false);
        let apostrophe = perform(
            json!({"type":"symbolRules", "key":"'", "chinese":false, "before":"don", "after":""}),
        )
        .unwrap();
        assert_eq!(apostrophe["pair"], false);
        let bracket = perform(json!({"type":"symbolRules", "key":"[", "chinese":true})).unwrap();
        assert_eq!(bracket["right"], "】");
        assert_eq!(bracket["pair"], true);
    }
}
