//! Settings persistence and blocking operations, independent of the UI.
use crate::statistics;
use retype_types::PinyinScheme;
use sha2::{Digest, Sha256};
use std::os::windows::process::CommandExt;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::*,
        System::Registry::*,
        UI::{Shell::*, WindowsAndMessaging::*},
    },
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub const KEY: &str = "Software\\retype";
pub fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

#[allow(unsafe_code)]
pub fn dword_at(key: &str, name: &str) -> Result<Option<u32>> {
    let key = wide(key);
    let name = wide(name);
    let mut value = 0u32;
    let mut size = 4;
    // SAFETY: Output is a DWORD, matching the advertised four-byte buffer.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status.ok()?;
    Ok(Some(value))
}
#[allow(unsafe_code)]
pub fn set_at(key: &str, name: &str, value: u32) -> Result<()> {
    let key_name = wide(key);
    let name = wide(name);
    let mut key = HKEY::default();
    // SAFETY: Valid null-terminated names and owned key handle; closed on all paths.
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let status = RegSetValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            REG_DWORD,
            Some(&value.to_le_bytes()),
        );
        let _ = RegCloseKey(key);
        status.ok()?;
    }
    Ok(())
}
pub fn set(name: &str, value: u32) -> Result<()> {
    set_at(KEY, name, value)
}
#[allow(unsafe_code)]
fn installed(name: &str) -> Result<Option<String>> {
    let name = wide(name);
    let mut data = vec![0u16; 32768];
    let mut size = (data.len() * 2) as u32;
    // SAFETY: Fixed UTF-16 buffer; always use the 64-bit installation registry view.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("Software\\retype"),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status.ok()?;
    let end = data.iter().position(|v| *v == 0).unwrap_or(data.len());
    Ok(Some(String::from_utf16_lossy(&data[..end])))
}
pub fn local_root() -> Result<PathBuf> {
    Ok(
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("无法找到本地数据目录")?)
            .join("retype"),
    )
}
#[derive(Clone)]
pub struct Preferences {
    pub scheme: PinyinScheme,
    pub auto_check: bool,
    pub pack_mask: u32,
    pub version: String,
    pub directory: PathBuf,
}
pub fn preferences() -> Result<Preferences> {
    let auto_check = match dword_at(KEY, "AutoCheck")? {
        Some(value) => value != 0,
        None => {
            let enabled = local_root()
                .ok()
                .and_then(|root| fs::read(root.join("updates/state.json")).ok())
                .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
                .and_then(|state| state["AutoCheck"].as_bool())
                .unwrap_or(true);
            set("AutoCheck", u32::from(enabled))?;
            enabled
        }
    };
    let directory = installed("ActiveDir")?
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf))
                .map(|dir| {
                    if dir.file_name().is_some_and(|name| name == "settings") {
                        dir.parent().unwrap_or(&dir).to_path_buf()
                    } else {
                        dir
                    }
                })
                .unwrap_or_default()
        });
    Ok(Preferences {
        scheme: if dword_at(KEY, "PinyinScheme")? == Some(1) {
            PinyinScheme::Flypy
        } else {
            PinyinScheme::Full
        },
        auto_check,
        pack_mask: dword_at(KEY, "EnabledDictionaryPacks")?.unwrap_or(0) & 0x7f,
        version: installed("Version")?.unwrap_or_else(|| env!("CARGO_PKG_VERSION").into()),
        directory,
    })
}
#[derive(Clone, serde::Deserialize)]
pub struct Pack {
    pub id: String,
    pub title: String,
    pub description: String,
    pub bytes: u64,
    pub sha256: String,
}
pub fn packs() -> Result<Vec<Pack>> {
    Ok(serde_json::from_str(include_str!(
        "../dictionary-packs.json"
    ))?)
}
pub fn checksum(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
pub fn pack_installed(root: &Path, pack: &Pack) -> bool {
    let expected = fs::read_to_string(root.join(format!("{}.sha256", pack.id))).unwrap_or_default();
    let expected = expected.trim();
    expected.len() == 64
        && checksum(&root.join(format!("{}.bin", pack.id))).is_ok_and(|value| value == expected)
}
fn pack_preference(index: usize, enabled: bool) -> Result<()> {
    if index >= 7 {
        return Err("无效词库".into());
    }
    let mask = dword_at(KEY, "EnabledDictionaryPacks")?.unwrap_or(0);
    set(
        "EnabledDictionaryPacks",
        if enabled {
            mask | (1 << index)
        } else {
            mask & !(1 << index)
        },
    )?;
    set(
        "DictionaryGeneration",
        dword_at(KEY, "DictionaryGeneration")?
            .unwrap_or(0)
            .wrapping_add(1),
    )
}
fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err("下载已取消".into())
    } else {
        Ok(())
    }
}
pub fn set_pack(
    index: usize,
    enabled: bool,
    cancel: &AtomicBool,
    progress: impl FnMut(f32),
) -> Result<()> {
    let packs = packs()?;
    let pack = packs.get(index).ok_or("无效词库")?;
    let root = local_root()?.join("dict-packs");
    if enabled && !pack_installed(&root, pack) {
        download_pack(
            &root,
            pack,
            &preferences()?.directory.join("retype-dict-build.exe"),
            cancel,
            progress,
        )?;
    }
    cancelled(cancel)?;
    pack_preference(index, enabled)
}
fn download_pack(
    root: &Path,
    pack: &Pack,
    builder: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<()> {
    fs::create_dir_all(root)?;
    let nonce = chrono::Local::now().timestamp_micros();
    let raw = root.join(format!("{}-{nonce}.yaml", pack.id));
    let tsv = raw.with_extension("tsv");
    let binary = raw.with_extension("bin");
    let result = (|| -> Result<()> {
        cancelled(cancel)?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(300)))
            .timeout_connect(Some(Duration::from_secs(20)))
            .build()
            .into();
        let url = format!(
            "https://raw.githubusercontent.com/amzxyz/rime-wanxiang/v18.0.14/dicts/{}.dict.yaml",
            pack.id
        );
        let mut response = agent.get(&url).call()?;
        let mut reader = response.body_mut().as_reader();
        let mut output = fs::File::create(&raw)?;
        let mut buffer = [0u8; 32768];
        let mut received = 0u64;
        loop {
            cancelled(cancel)?;
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            received += count as u64;
            if received > pack.bytes || received > 8 * 1024 * 1024 {
                return Err("词库超过大小限制".into());
            }
            output.write_all(&buffer[..count])?;
            progress(received as f32 / pack.bytes as f32);
        }
        output.sync_all()?;
        drop(output);
        if received != pack.bytes || checksum(&raw)? != pack.sha256 {
            return Err("词库校验失败，请重试".into());
        }
        cancelled(cancel)?;
        let result = Command::new(builder)
            .args(["--in"])
            .arg(&raw)
            .arg("--out")
            .arg(&tsv)
            .args(["--max-word-len", "8", "--no-verify"])
            .creation_flags(0x08000000)
            .output()?;
        if !result.status.success() || !binary.is_file() {
            return Err(
                format!("词库转换失败：{}", String::from_utf8_lossy(&result.stderr)).into(),
            );
        }
        cancelled(cancel)?;
        let digest = checksum(&binary)?;
        replace(&binary, &root.join(format!("{}.bin", pack.id)))?;
        let checksum_path = root.join(format!("{}.sha256", pack.id));
        let staging = checksum_path.with_extension("sha256.tmp");
        fs::write(&staging, digest)?;
        replace(&staging, &checksum_path)?;
        Ok(())
    })();
    for path in [&raw, &tsv, &binary] {
        let _ = fs::remove_file(path);
    }
    result
}
#[allow(unsafe_code)]
fn replace(source: &Path, target: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::*;
    let source: Vec<_> = source.as_os_str().encode_wide().chain([0]).collect();
    let target: Vec<_> = target.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: Both are null-terminated paths under the owned data directory.
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(target.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )?;
    }
    Ok(())
}
#[derive(Clone)]
pub struct Offer {
    pub available: bool,
    pub installable: bool,
    pub version: Option<String>,
    pub url: Option<String>,
}
fn updater(arguments: &[&str]) -> Result<serde_json::Value> {
    let output = Command::new(preferences()?.directory.join("retype-updater.exe"))
        .args(arguments)
        .creation_flags(0x08000000)
        .output()?;
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    if !matches!(output.status.code(), Some(0 | 10)) {
        return Err(value["error"]
            .as_str()
            .unwrap_or("更新器失败")
            .to_string()
            .into());
    }
    Ok(value)
}
pub fn check() -> Result<Offer> {
    let prefs = preferences()?;
    let result = updater(&[
        "check",
        "--repo",
        "L-Chris/retype",
        "--current",
        &prefs.version,
        "--json",
    ])?;
    Ok(Offer {
        available: result["update_available"].as_bool().unwrap_or(false),
        installable: result["installable"].as_bool().unwrap_or(false),
        version: result["latest"]["version"].as_str().map(str::to_owned),
        url: result["release_page"].as_str().map(str::to_owned),
    })
}
pub fn skip(version: &str) -> Result<()> {
    skip_at(&local_root()?.join("updates"), version)
}
fn skip_at(root: &Path, version: &str) -> Result<()> {
    fs::create_dir_all(root)?;
    let path = root.join("state.json");
    let mut state = fs::read(&path)
        .ok()
        .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    state["SkippedVersion"] = version.into();
    let staging = root.join(format!("state-{}.tmp", std::process::id()));
    fs::write(&staging, serde_json::to_vec(&state)?)?;
    replace(&staging, &path)
}
#[allow(unsafe_code)]
pub fn open(destination: &str) -> Result<()> {
    let destination = wide(destination);
    // SAFETY: Fixed action and a null-terminated destination, no shell command string.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(destination.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize <= 32 {
        return Err("无法打开目标".into());
    }
    Ok(())
}
pub fn install(version: &str, mut stage: impl FnMut(&str)) -> Result<String> {
    let root = local_root()?
        .join("updates")
        .join(chrono::Local::now().timestamp_micros().to_string());
    fs::create_dir_all(&root)?;
    let directory = root.to_str().ok_or("更新目录编码无效")?;
    stage("正在下载并校验安装包…");
    let value = updater(&[
        "download",
        "--repo",
        "L-Chris/retype",
        "--json",
        "--timeout",
        "300",
        "--expected-version",
        version,
        "--out",
        directory,
    ])?;
    let installer = PathBuf::from(value["downloaded"].as_str().ok_or("更新器没有返回安装包")?);
    if !installer.is_file()
        || installer.extension().is_none_or(|v| v != "exe")
        || !installer.starts_with(&root)
    {
        return Err("无效安装包路径".into());
    }
    stage("正在安装；如弹出管理员权限提示，请确认。");
    let code = elevate(&installer)?;
    if code == 3010 {
        return Ok("安装已准备完成，请手动重启电脑后生效。".into());
    }
    if code != 0 {
        return Err(format!("安装失败，退出码：{code}").into());
    }
    stage("正在验证安装…");
    let prefs = preferences()?;
    let result = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(prefs.directory.join("update-verify.ps1"))
        .args(["-ExpectedVersion", version])
        .creation_flags(0x08000000)
        .output()?;
    if !result.status.success() {
        return Err(format!(
            "安装后验证失败：{}",
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    Ok(format!(
        "已安装 {version}。重新打开正在运行的应用即可使用新版。"
    ))
}
#[allow(unsafe_code)]
fn elevate(installer: &Path) -> Result<u32> {
    use windows::Win32::System::Threading::*;
    let file = wide(&installer.to_string_lossy());
    // SAFETY: All strings outlive ShellExecuteExW; process handle closed on every path.
    unsafe {
        let mut info=SHELLEXECUTEINFOW { cbSize:std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask:SEE_MASK_NOCLOSEPROCESS,lpVerb:w!("runas"),lpFile:PCWSTR(file.as_ptr()),
            lpParameters:w!("/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /NOCLOSEAPPLICATIONS /NORESTARTAPPLICATIONS /RESTARTEXITCODE=3010"),nShow:SW_HIDE.0,..Default::default() };
        ShellExecuteExW(&mut info)?;
        if info.hProcess.is_invalid() {
            return Err("安装器没有返回进程".into());
        }
        let wait = WaitForSingleObject(info.hProcess, 600_000);
        let mut code = 0;
        let result: Result<()> = if wait == WAIT_OBJECT_0 {
            GetExitCodeProcess(info.hProcess, &mut code).map_err(Into::into)
        } else {
            Err("安装等待超时，请检查安装日志".into())
        };
        let _ = CloseHandle(info.hProcess);
        result?;
        Ok(code)
    }
}

pub enum Task {
    Packs,
    SetPack(usize, bool, Arc<AtomicBool>),
    Statistics,
    Check,
    Install(String),
    Skip(String),
}
pub enum Event {
    Packs(Vec<bool>),
    PackProgress(f32),
    PackDone(std::result::Result<(), String>),
    Statistics(statistics::Snapshot),
    Checked(std::result::Result<Offer, String>),
    UpdateStage(String),
    Installed(std::result::Result<String, String>),
    Skipped(std::result::Result<(), String>),
    Error(String),
}
pub fn worker(
    ctx: eframe::egui::Context,
) -> (
    std::sync::mpsc::Sender<Task>,
    std::sync::mpsc::Receiver<Event>,
    Arc<AtomicBool>,
) {
    let (tasks, receiver) = std::sync::mpsc::channel();
    let (sender, events) = std::sync::mpsc::channel();
    let busy = Arc::new(AtomicBool::new(false));
    let worker_busy = Arc::clone(&busy);
    std::thread::spawn(move || {
        let root = local_root().unwrap_or_else(|_| std::env::temp_dir().join("retype"));
        let mut statistics = statistics::Store::new(root.join("statistics"));
        let emit = |event| {
            let _ = sender.send(event);
            ctx.request_repaint();
        };
        for task in receiver {
            struct BusyGuard(Arc<AtomicBool>);
            impl Drop for BusyGuard {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::Relaxed);
                }
            }
            worker_busy.store(true, Ordering::Relaxed);
            let _guard = BusyGuard(Arc::clone(&worker_busy));
            match task {
                Task::Packs => match packs() {
                    Ok(packs) => emit(Event::Packs(
                        packs
                            .iter()
                            .map(|pack| pack_installed(&root.join("dict-packs"), pack))
                            .collect(),
                    )),
                    Err(e) => emit(Event::Error(e.to_string())),
                },
                Task::SetPack(index, enabled, cancel) => emit(Event::PackDone(
                    set_pack(index, enabled, &cancel, |p| emit(Event::PackProgress(p)))
                        .map_err(|e| e.to_string()),
                )),
                Task::Statistics => {
                    let now = chrono::Local::now().timestamp_millis();
                    match statistics.load(now) {
                        Ok(data) => emit(Event::Statistics(data)),
                        Err(e) => emit(Event::Error(e.to_string())),
                    }
                }
                Task::Check => emit(Event::Checked(check().map_err(|e| e.to_string()))),
                Task::Install(version) => emit(Event::Installed(
                    install(&version, |text| emit(Event::UpdateStage(text.into())))
                        .map_err(|e| e.to_string()),
                )),
                Task::Skip(version) => {
                    emit(Event::Skipped(skip(&version).map_err(|e| e.to_string())))
                }
            }
        }
    });
    (tasks, events, busy)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_matches_existing_tsf_pack_bits() -> Result<()> {
        let packs = packs()?;
        assert_eq!(
            packs
                .iter()
                .map(|pack| pack.id.as_str())
                .collect::<Vec<_>>(),
            ["mingren", "renming", "yiren", "diming", "yixue", "yaopin", "huaxue"]
        );
        assert!(packs
            .iter()
            .all(|pack| pack.bytes > 0 && pack.bytes < 8 * 1024 * 1024 && pack.sha256.len() == 64));
        Ok(())
    }
    #[test]
    fn checksum_detects_corruption_and_skip_keeps_existing_state() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("retype-egui-backend-test-{}", std::process::id()));
        fs::create_dir_all(&root)?;
        let pack = packs()?.remove(0);
        let binary = root.join(format!("{}.bin", pack.id));
        fs::write(&binary, b"test dictionary")?;
        fs::write(root.join(format!("{}.sha256", pack.id)), checksum(&binary)?)?;
        assert!(pack_installed(&root, &pack));
        fs::write(&binary, b"corrupt")?;
        assert!(!pack_installed(&root, &pack));
        fs::write(
            root.join("state.json"),
            br#"{"AutoCheck":false,"LastCheck":"retained"}"#,
        )?;
        skip_at(&root, "0.5.0")?;
        let state: serde_json::Value = serde_json::from_slice(&fs::read(root.join("state.json"))?)?;
        assert_eq!(state["AutoCheck"], false);
        assert_eq!(state["LastCheck"], "retained");
        assert_eq!(state["SkippedVersion"], "0.5.0");
        for entry in fs::read_dir(&root)? {
            fs::remove_file(entry?.path())?;
        }
        fs::remove_dir(root)?;
        Ok(())
    }
    #[test]
    #[ignore = "downloads the pinned upstream pack and needs RETYPE_TEST_DICT_BUILDER"]
    fn download_and_convert_without_changing_user_preferences() -> Result<()> {
        let builder = PathBuf::from(
            std::env::var_os("RETYPE_TEST_DICT_BUILDER")
                .ok_or("Missing test dictionary builder")?,
        );
        let root =
            std::env::temp_dir().join(format!("retype-pack-egui-smoke-{}", std::process::id()));
        let pack = packs()?.remove(0);
        let result = download_pack(&root, &pack, &builder, &AtomicBool::new(false), |_| {});
        if result.is_ok() {
            assert!(pack_installed(&root, &pack));
        }
        if root.exists() {
            for entry in fs::read_dir(&root)? {
                fs::remove_file(entry?.path())?;
            }
            fs::remove_dir(root)?;
        }
        result
    }
}
