//! All cloud and broker IO runs in a full-trust per-user helper, never a TIP.
use crate::{
    config::{self, Config},
    model::{Snapshot, State},
    statistics::{self, Bucket},
    webdav::WebDav,
    Result,
};
use retype_learning::{
    protocol::{SyncLearning, SyncRequest},
    transport,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{Foundation::*, System::Registry::*};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub busy: bool,
    pub message: String,
    pub last_success: u64,
    pub awaiting_merge: bool,
    pub devices: usize,
    pub words: usize,
    pub statistics: usize,
    pub conflicts: Vec<crate::model::Conflict>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum Command {
    Sync,
    Confirm,
    Resolve {
        key: String,
        remote: bool,
        stamp: crate::model::Stamp,
    },
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn account_root(root: &Path, config: &Config) -> PathBuf {
    root.join(config.account())
}
fn load<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).map_err(|_| "本地同步状态损坏".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(_) => Err("无法读取本地同步状态".into()),
    }
}
pub fn status(root: &Path, config: &Config) -> Result<Status> {
    load(&account_root(root, config).join("status.json"))
}
pub fn password(config: &Config) -> Result<String> {
    retype_ai::secrets::cloud_password(&config.account()).map_err(|_| "无法读取云盘应用密码".into())
}
pub fn save(config: &Config, password_text: &str) -> Result<()> {
    if config.enabled {
        config.validate()?;
    }
    let root = config::root()?;
    transport::private_directory(
        &root,
        &transport::user_sid().map_err(|_| "无法识别当前用户")?,
    )
    .map_err(|_| "无法设置同步目录权限")?;
    let stored = password(config)?;
    if stored != password_text {
        retype_ai::secrets::save_cloud_password(&config.account(), password_text)
            .map_err(|_| "无法保存云盘应用密码")?;
    }
    config.save_at(&root)?;
    if config.enabled {
        launch()?
    }
    Ok(())
}
pub fn launch() -> Result<()> {
    let helper = transport::read_machine_registry("ActiveDir")
        .map(PathBuf::from)
        .map(|p| p.join("retype-sync-host.exe"))
        .filter(|p| p.is_file())
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join("retype-sync-host.exe")))
                .filter(|p| p.is_file())
        })
        .ok_or("找不到云同步后台，请安装包含云同步功能的版本")?;
    retype_learning::client::launch_host(Some(&helper)).map_err(|_| "无法启动云同步后台".into())
}
pub fn command(config: &Config, command: Command) -> Result<()> {
    config::write_json(
        &account_root(&config::root()?, config).join("command.json"),
        &command,
    )?;
    launch()
}
fn broker(request: &SyncRequest) -> Result<SyncLearning> {
    let name = transport::pipe_name(&transport::user_sid().map_err(|_| "无法识别当前用户")?);
    let bytes = serde_json::to_vec(request).map_err(|_| "无法编码学习同步请求")?;
    let response =
        transport::exchange(&name, &bytes).map_err(|_| "学习后台未就绪，请重新打开输入法后同步")?;
    serde_json::from_slice(&response).map_err(|_| "学习后台不支持云同步，请更新完整安装包".into())
}
#[allow(unsafe_code)]
fn registry(name: &str) -> Result<Option<u32>> {
    let mut value = 0u32;
    let mut len = 4u32;
    // SAFETY: Fixed per-user preference path and bounded DWORD storage.
    let code = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            transport::wide(r"Software\retype").as_ptr(),
            transport::wide(name).as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut len,
        )
    };
    match code {
        ERROR_SUCCESS => Ok(Some(value)),
        ERROR_FILE_NOT_FOUND => Ok(None),
        _ => Err("无法读取输入法设置".into()),
    }
}
#[allow(unsafe_code)]
fn set_registry(name: &str, value: u32) -> Result<()> {
    let mut key = std::ptr::null_mut();
    // SAFETY: Fixed preference path, owned handle, synchronous DWORD copy.
    unsafe {
        let code = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            transport::wide(r"Software\retype").as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        );
        if code != ERROR_SUCCESS {
            return Err("无法打开输入法设置".into());
        }
        let code = RegSetValueExW(
            key,
            transport::wide(name).as_ptr(),
            0,
            REG_DWORD,
            (&value as *const u32).cast(),
            4,
        );
        RegCloseKey(key);
        if code != ERROR_SUCCESS {
            return Err("无法保存同步后的设置".into());
        }
    }
    Ok(())
}
fn portable() -> Result<BTreeMap<String, Value>> {
    let ai = retype_ai::config::Config::load().map_err(|_| "无法读取 AI 设置")?;
    let shortcuts = retype_ai::secrets::shortcuts();
    let mut values = BTreeMap::from([
        (
            "input.scheme".into(),
            json!(registry("PinyinScheme")?.unwrap_or(0)),
        ),
        (
            "input.english".into(),
            json!(registry("EnglishDisabled")?.unwrap_or(0) == 0),
        ),
        (
            "input.english_spelling".into(),
            json!(registry("EnglishSpellingDisabled")?.unwrap_or(0) == 0),
        ),
        (
            "dictionary.enabled".into(),
            json!(registry("EnabledDictionaryPacks")?.unwrap_or(0) & 0x7f),
        ),
        (
            "updates.auto_check".into(),
            json!(registry("AutoCheck")?.unwrap_or(1) != 0),
        ),
        ("shortcuts".into(), json!(shortcuts)),
        (
            "translation.model".into(),
            json!({"provider":ai.provider,"model":ai.model}),
        ),
        ("translation.target".into(), json!(ai.target)),
        ("translation.preview".into(), json!(ai.preview)),
        ("translation.instructions".into(), json!(ai.instructions)),
        ("translation.reasoning".into(), json!(ai.reasoning)),
        ("translation.timeout".into(), json!(ai.timeout_seconds)),
    ]);
    for provider in ai.providers {
        if provider.base_url.contains(['@', '?', '#']) {
            return Err("AI 提供商地址包含凭据或查询参数，请先在 AI 提供商页面修正".into());
        }
        values.insert(format!("providers/{}", provider.id), json!(provider));
    }
    Ok(values)
}
fn validate_snapshot(snapshot: &Snapshot) -> Result<()> {
    if !snapshot.learning.valid() {
        return Err("云端学习数据无效，本地数据未应用".into());
    }
    let mut checked = Vec::new();
    statistics::merge(&mut checked, &snapshot.statistics)?;
    if snapshot.version != 1
        || !config::valid_id(&snapshot.device)
        || snapshot.name.len() > 256
        || snapshot.fields.len() > 2000
    {
        return Err("云端版本不兼容或数据规模超过限制".into());
    }
    for (key, field) in &snapshot.fields {
        if !known_key(key)
            || field.ancestors.len() > 20000
            || field.stamp.revision == u64::MAX
            || !config::valid_id(&field.stamp.device)
            || field.ancestors.iter().any(|s| !config::valid_id(&s.device))
        {
            return Err("云端设置版本无效".into());
        }
    }
    Ok(())
}
fn known_key(key: &str) -> bool {
    matches!(
        key,
        "input.scheme"
            | "dictionary.enabled"
            | "updates.auto_check"
            | "shortcuts"
            | "translation.model"
            | "translation.target"
            | "translation.preview"
            | "translation.instructions"
            | "translation.reasoning"
            | "translation.timeout"
    ) || key
        .strip_prefix("providers/")
        .is_some_and(|s| !s.is_empty() && s.len() <= 128 && !s.contains(['/', '\\']))
}
fn apply_values(values: &BTreeMap<String, Value>) -> Result<()> {
    let mut ai = retype_ai::config::Config::load().map_err(|_| "无法读取本机 AI 设置")?;
    let shortcuts: retype_ai::config::Shortcuts =
        serde_json::from_value(values.get("shortcuts").ok_or("缺少快捷键设置")?.clone())
            .map_err(|_| "快捷键格式无效")?;
    shortcuts.validate()?;
    let scheme = values
        .get("input.scheme")
        .and_then(Value::as_u64)
        .filter(|v| *v <= 1)
        .ok_or("拼音方案无效")? as u32;
    let mask = values
        .get("dictionary.enabled")
        .and_then(Value::as_u64)
        .filter(|v| *v <= 0x7f)
        .ok_or("词库设置无效")? as u32;
    let english = values
        .get("input.english")
        .map(Value::as_bool)
        .unwrap_or(Some(true))
        .ok_or("英文输入设置无效")?;
    let spelling = values
        .get("input.english_spelling")
        .map(Value::as_bool)
        .unwrap_or(Some(true))
        .ok_or("英文拼写设置无效")?;
    let auto = values
        .get("updates.auto_check")
        .and_then(Value::as_bool)
        .ok_or("更新设置无效")?;
    ai.providers = values
        .iter()
        .filter(|(k, v)| k.starts_with("providers/") && !v.is_null())
        .map(|(key, v)| {
            let p: retype_ai::config::Provider =
                serde_json::from_value(v.clone()).map_err(|_| "AI 提供商设置无效")?;
            if key != &format!("providers/{}", p.id)
                || p.models.len() > 500
                || p.models.iter().any(|m| m.len() > 512)
                || p.base_url.len() > 2048
                || p.name.len() > 256
                || p.preset.len() > 128
            {
                return Err("AI 提供商设置超出限制");
            }
            Ok(p)
        })
        .collect::<std::result::Result<_, &str>>()
        .map_err(String::from)?;
    let selection = values.get("translation.model").ok_or("缺少翻译模型设置")?;
    ai.provider = selection["provider"]
        .as_str()
        .ok_or("翻译模型选择无效")?
        .into();
    ai.model = selection["model"]
        .as_str()
        .ok_or("翻译模型选择无效")?
        .into();
    ai.target = values
        .get("translation.target")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty() && v.len() < 256)
        .ok_or("目标语言无效")?
        .into();
    ai.instructions = values
        .get("translation.instructions")
        .and_then(Value::as_str)
        .filter(|v| v.len() <= 65536)
        .ok_or("翻译要求无效")?
        .into();
    ai.reasoning = values
        .get("translation.reasoning")
        .and_then(Value::as_str)
        .filter(|v| {
            matches!(
                *v,
                "default" | "none" | "minimal" | "low" | "medium" | "high"
            )
        })
        .ok_or("思考等级无效")?
        .into();
    ai.preview = values
        .get("translation.preview")
        .and_then(Value::as_bool)
        .ok_or("翻译结果处理无效")?;
    ai.timeout_seconds = values
        .get("translation.timeout")
        .and_then(Value::as_u64)
        .filter(|v| (5..=180).contains(v))
        .ok_or("翻译超时无效")?;
    // All validation precedes writes; preserve the old portable values for recovery.
    ai.save().map_err(|_| "无法保存同步后的 AI 设置")?;
    retype_ai::secrets::save_shortcuts(shortcuts).map_err(|_| "无法保存同步后的快捷键")?;
    let previous_mask = registry("EnabledDictionaryPacks")?.unwrap_or(0);
    set_registry("PinyinScheme", scheme)?;
    set_registry("EnglishDisabled", u32::from(!english))?;
    set_registry("EnglishSpellingDisabled", u32::from(!spelling))?;
    set_registry("EnabledDictionaryPacks", mask)?;
    if previous_mask != mask {
        set_registry(
            "DictionaryGeneration",
            registry("DictionaryGeneration")?
                .unwrap_or(0)
                .wrapping_add(1),
        )?;
    }
    set_registry("AutoCheck", u32::from(auto))
}
fn cycle(root: &Path, config: &Config, state: &mut State, status: &mut Status) -> Result<()> {
    let cloud = WebDav::new(config, &password(config)?)?;
    let account = account_root(root, config);
    let data_root = root.parent().ok_or("数据目录无效")?;
    let current = portable()?;
    let mut base = current.clone();
    if state.initialized {
        // Explicit tombstones preserve deleted providers across devices.
        let mut values = current.clone();
        for key in state
            .fields
            .keys()
            .filter(|k| k.starts_with("providers/") && !current.contains_key(*k))
        {
            values.insert(key.clone(), Value::Null);
        }
        state.capture(&config.device_id, &values);
        config::write_json(&account.join("state.json"), state)?;
    }
    cloud.ensure_layout()?;
    let mut remote = Vec::<Snapshot>::new();
    for id in cloud.devices()? {
        let snapshot: Snapshot = cloud.download_cached(&id, &account.join("cache"))?;
        validate_snapshot(&snapshot)?;
        if snapshot.device != id {
            return Err("云端设备身份不匹配".into());
        }
        remote.push(snapshot);
    }
    status.devices = remote.len();
    status.words = remote.iter().map(|s| s.learning.words.len()).sum();
    status.statistics = remote.iter().map(|s| s.statistics.len()).sum();
    if !state.initialized && !state.confirmed && !remote.is_empty() {
        status.awaiting_merge = true;
        status.message = "发现已有云端数据，请确认首次合并".into();
        return Ok(());
    }
    if Config::load_at(root)? != *config {
        return Err("云盘配置已变化，本次同步已停止".into());
    }
    if state.initialized {
        // Changes made while downloading are captured before merging.
        let mut fresh = portable()?;
        base.clone_from(&fresh);
        let removed: Vec<_> = state
            .fields
            .keys()
            .filter(|k| k.starts_with("providers/") && !fresh.contains_key(*k))
            .cloned()
            .collect();
        for key in removed {
            fresh.insert(key, Value::Null);
        }
        state.capture(&config.device_id, &fresh);
    }
    for snapshot in &remote {
        state.merge(snapshot);
    }
    if !state.initialized {
        let missing: BTreeMap<_, _> = current
            .iter()
            .filter(|(k, _)| !state.fields.contains_key(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        state.capture(&config.device_id, &missing);
        state.initialized = true;
    }
    let values = state.values();
    let mut comparable = base.clone();
    for (key, value) in &values {
        if key.starts_with("providers/") && value.is_null() {
            comparable.insert(key.clone(), Value::Null);
        }
    }
    if values != comparable {
        if portable()? != base {
            return Err("本机设置正在修改，本次未覆盖设置，将稍后重试".into());
        }
        config::write_json(&account.join("before-merge.json"), &base)?;
        apply_values(&values)?;
    }
    let mut imported: Vec<Bucket> = load(&account.join("statistics.json"))?;
    for snapshot in &remote {
        broker(&SyncRequest::Merge(snapshot.learning.clone()))?;
        statistics::merge(&mut imported, &snapshot.statistics)?;
    }
    let local = statistics::capture(&data_root.join("statistics"), &config.device_id)?;
    statistics::merge(&mut imported, &local)?;
    config::write_json(&account.join("statistics.json"), &imported)?;
    // Add remote history and restored evidence missing from local writer files.
    let history = statistics::history(&imported, &local, &config.device_id);
    config::write_json(&data_root.join("statistics/cloud-history.json"), &history)?;
    let learning = broker(&SyncRequest::Export)?;
    let snapshot = Snapshot {
        version: 1,
        device: config.device_id.clone(),
        name: config.device_name.clone(),
        fields: state.fields.clone(),
        learning,
        statistics: imported,
    };
    cloud.publish(&config.device_id, &snapshot)?;
    config::write_json(&account.join("state.json"), state)?;
    status.awaiting_merge = false;
    status.conflicts = state.conflicts.clone();
    status.last_success = now();
    // Optional dictionaries remain separate downloads, including on a new device.
    if let Some(active) = transport::read_machine_registry("ActiveDir") {
        let installed = PathBuf::from(active);
        let settings = installed.join("settings/retype.exe");
        let current = std::env::current_exe().ok();
        let settings = if installed.join("retype-sync-host.exe").is_file() {
            settings
        } else {
            current
                .and_then(|p| p.parent().map(|p| p.join("retype-settings-egui.exe")))
                .unwrap_or(settings)
        };
        if settings.is_file() {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new(settings)
                .arg("--sync-dictionaries")
                .creation_flags(0x08000000)
                .spawn();
        }
    }
    status.message = if state.conflicts.is_empty() {
        "同步完成".into()
    } else {
        "数据已同步，有设置冲突需要处理".into()
    };
    Ok(())
}

pub fn serve() -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let root = config::root()?;
    let initial = Config::load_at(&root)?;
    if !initial.enabled {
        return Ok(());
    }
    transport::private_directory(
        &root,
        &transport::user_sid().map_err(|_| "无法识别当前用户")?,
    )
    .map_err(|_| "无法设置同步目录权限")?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(root.join("host.lock"));
    let mut lock = match lock {
        Ok(file) => Some(file),
        Err(e) if matches!(e.raw_os_error(), Some(32 | 33)) => return Ok(()),
        Err(_) => return Err("无法获得云同步后台锁".into()),
    };
    let mut next = Instant::now();
    let mut retries = 0u32;
    let mut last_account = initial.account();
    let stop = root.join("stop");
    let _ = std::fs::remove_file(&stop);
    loop {
        if stop.is_file() {
            let _ = std::fs::remove_file(&stop);
            return Ok(());
        }
        let config = Config::load_at(&root)?;
        if !config.enabled {
            return Ok(());
        }
        if config.account() != last_account {
            last_account = config.account();
            next = Instant::now();
            retries = 0;
        }
        let directory = std::env::current_exe()
            .map_err(|_| "无法识别后台版本")?
            .parent()
            .ok_or("后台目录无效")?
            .to_path_buf();
        if let Some(active) = transport::read_machine_registry("ActiveDir") {
            let path = PathBuf::from(active).join("retype-sync-host.exe");
            if path.is_file() && path.parent() != Some(directory.as_path()) {
                drop(lock.take());
                retype_learning::client::launch_host(Some(&path))
                    .map_err(|_| "无法切换同步后台版本")?;
                return Ok(());
            }
        }
        let account = account_root(&root, &config);
        let command_path = account.join("command.json");
        let command = std::fs::read(&command_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Command>(&b).ok());
        if command.is_some() {
            let _ = std::fs::remove_file(&command_path);
        }
        if Instant::now() >= next || command.is_some() {
            let mut state: State = load(&account.join("state.json"))?;
            // Resolve only after recording edits made since the last sync.
            if matches!(&command, Some(Command::Resolve { .. })) {
                state.capture(&config.device_id, &portable()?);
            }
            let mut status = status(&root, &config)?;
            match command {
                Some(Command::Confirm) => state.confirmed = true,
                Some(Command::Resolve { key, remote, stamp })
                    if state.resolve(&config.device_id, &key, remote, Some(&stamp)) =>
                {
                    apply_values(&state.values())?;
                    config::write_json(&account.join("state.json"), &state)?;
                }
                _ => {}
            }
            status.busy = true;
            status.message = "同步中…".into();
            config::write_json(&account.join("status.json"), &status)?;
            let result = cycle(&root, &config, &mut state, &mut status);
            status.busy = false;
            if let Err(error) = result {
                status.message = error;
                retries = retries.saturating_add(1);
            } else {
                retries = 0;
            }
            status.conflicts = state.conflicts.clone();
            config::write_json(&account.join("state.json"), &state)?;
            config::write_json(&account.join("status.json"), &status)?;
            let delay = if retries == 0 {
                300
            } else {
                match retries {
                    1 => 30,
                    2 => 120,
                    _ => 600,
                }
            };
            next = Instant::now() + Duration::from_secs(delay);
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}
