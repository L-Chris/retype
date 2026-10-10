//! Native daily update checks. No recording, installation or shell processes.
use chrono::{DateTime, Utc};
use retype_updater_cli::installation::{self, Result};
use serde_json::Value;
use std::{fs, path::Path};

fn load(path: &Path) -> Value {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}))
}

fn due(state: &Value, preference: Option<bool>, now: DateTime<Utc>) -> bool {
    if !preference.unwrap_or_else(|| state["AutoCheck"].as_bool().unwrap_or(true)) {
        return false;
    }
    state["LastCheck"]
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .is_none_or(|last| now.signed_duration_since(last).num_seconds() >= 86400)
}

#[allow(unsafe_code)]
fn save(path: &Path, state: &Value) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{core::PCWSTR, Win32::Storage::FileSystem::*};
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, serde_json::to_vec(state)?)?;
    let source: Vec<_> = temporary.as_os_str().encode_wide().chain([0]).collect();
    let target: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: Both terminated paths belong to the update state directory.
    let result = unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(target.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(())
}

fn check_and_remind(
    path: &Path,
    preference: Option<bool>,
    now: DateTime<Utc>,
    check: impl FnOnce() -> Result<retype_updater::UpdateStatus>,
    remind: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let mut state = load(path);
    if !due(&state, preference, now) {
        return Ok(());
    }
    state["LastCheck"] = now.to_rfc3339().into();
    save(path, &state)?;
    let result: Result<()> = (|| {
        let status = check()?;
        // A user may skip this version from Settings while the network request runs.
        state = load(path);
        state["Error"] = "".into();
        save(path, &state)?;
        if status.update_available
            && status.latest.as_ref().is_some_and(|release| {
                state["SkippedVersion"].as_str() != Some(release.version.to_string().as_str())
            })
        {
            remind()?;
        }
        Ok(())
    })();
    if let Err(error) = &result {
        state = load(path);
        state["Error"] = error.to_string().into();
        save(path, &state)?;
    }
    result
}

pub fn run() -> Result<()> {
    use windows::{
        core::w,
        Win32::{Foundation::*, System::Threading::*},
    };
    struct Guard(HANDLE);
    #[allow(unsafe_code)]
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    // SAFETY: Per-logon-session mutex, owned handle released even on early exit.
    #[allow(unsafe_code)]
    let (mutex, already_running) = unsafe {
        let mutex = CreateMutexW(None, false, w!("Local\\retype-native-update-check"))?;
        (Guard(mutex), GetLastError() == ERROR_ALREADY_EXISTS)
    };
    let _guard = mutex;
    if already_running {
        return Ok(());
    }
    let root = std::path::PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is missing")?,
    )
    .join("retype/updates");
    fs::create_dir_all(&root)?;
    let directory = installation::active_directory()?;
    let current = installation::installed("Version")?;
    check_and_remind(
        &root.join("state.json"),
        installation::auto_check()?,
        Utc::now(),
        || {
            let checker = retype_updater::UpdateChecker::new(
                "L-Chris/retype",
                std::sync::Arc::new(crate::UreqFetcher::new(
                    std::time::Duration::from_secs(15),
                    None,
                )),
            );
            Ok(checker.check(&current, retype_updater::Platform::WindowsX64)?)
        },
        || {
            if installation::auto_check()? == Some(false) {
                return Ok(());
            }
            std::process::Command::new(directory.join("settings/retype.exe"))
                .arg("--updates")
                .spawn()?;
            Ok(())
        },
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn registry_preference_and_daily_limit_override_legacy_state() {
        let now = Utc::now();
        let mut state = serde_json::json!({"AutoCheck":false,"LastCheck":"invalid"});
        assert!(!due(&state, None, now));
        assert!(due(&state, Some(true), now));
        state["LastCheck"] = now.to_rfc3339().into();
        assert!(!due(&state, Some(true), now));
        assert!(due(&state, Some(true), now + chrono::Duration::hours(25)));
        assert!(!due(&state, Some(false), now + chrono::Duration::hours(25)));
    }

    #[test]
    fn native_check_preserves_state_skips_versions_and_recovers_after_errors() {
        let directory = std::env::temp_dir().join(format!(
            "retype-native-update-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("state.json");
        let now = Utc::now();
        let status = || {
            retype_updater::UpdateStatus {
            current: retype_updater::parse_version("0.7.2").unwrap(),
            latest: Some(retype_updater::parse_release_json(br#"{"tag_name":"v0.7.3","html_url":"https://github.com/L-Chris/retype/releases/tag/v0.7.3","assets":[]}"#).unwrap()),
            update_available: true, release_page: None, asset: None, checksum_asset: None,
        }
        };
        let checks = std::cell::Cell::new(0);
        let reminders = std::cell::Cell::new(0);
        save(&path, &serde_json::json!({"Stage":"installing","TargetVersion":"retained","SkippedVersion":"0.7.3"})).unwrap();
        check_and_remind(
            &path,
            Some(true),
            now,
            || {
                checks.set(checks.get() + 1);
                Ok(status())
            },
            || {
                reminders.set(reminders.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!((checks.get(), reminders.get()), (1, 0));
        check_and_remind(
            &path,
            Some(true),
            now,
            || {
                checks.set(checks.get() + 1);
                Ok(status())
            },
            || {
                reminders.set(reminders.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(checks.get(), 1);
        let state = load(&path);
        assert_eq!(state["Stage"], "installing");
        assert_eq!(state["TargetVersion"], "retained");
        assert_eq!(state["SkippedVersion"], "0.7.3");
        let later = now + chrono::Duration::hours(25);
        assert!(check_and_remind(
            &path,
            Some(true),
            later,
            || Err("offline fixture".into()),
            || Ok(())
        )
        .is_err());
        assert_eq!(load(&path)["Error"], "offline fixture");
        check_and_remind(
            &path,
            Some(true),
            later + chrono::Duration::hours(25),
            || {
                let mut state = load(&path);
                state["SkippedVersion"] = "".into();
                save(&path, &state)?;
                Ok(status())
            },
            || {
                reminders.set(reminders.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(reminders.get(), 1);
        assert_eq!(load(&path)["Error"], "");
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
