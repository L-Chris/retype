//! Fixed settings launch request for restricted input hosts. No caller-supplied
//! executable path or command line crosses the authenticated learning pipe.
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub start_settings: bool,
    pub request_id: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub settings_started: bool,
    pub win32: Option<i32>,
}

/// Worker only. The foreground process delegates its explicit menu action to
/// the same-user broker; a cold Settings process handles its own single instance.
pub fn open(request_id: u32) -> io::Result<()> {
    let sid = crate::transport::user_sid()?;
    let bytes = serde_json::to_vec(&Request {
        start_settings: true,
        request_id,
    })?;
    let response =
        crate::transport::exchange_with_foreground(&crate::transport::pipe_name(&sid), &bytes)?;
    let response: Response = serde_json::from_slice(&response)?;
    if response.settings_started {
        Ok(())
    } else {
        Err(response.win32.map_or_else(
            || io::ErrorKind::PermissionDenied.into(),
            io::Error::from_raw_os_error,
        ))
    }
}

#[cfg(feature = "broker")]
pub fn launch(directory: &std::path::Path, request_id: u32) -> io::Result<()> {
    if !directory.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    // The directory is selected by the broker from its own installation and
    // HKLM, never by a pipe client. Arguments consist only of a numeric log ID.
    let mut child = std::process::Command::new(directory.join("settings/retype.exe"))
        .arg(format!("--request-id={request_id}"))
        .current_dir(directory)
        .spawn()?;
    let pid = child.id();
    // SAFETY: this child was just created by the broker. The client delegated
    // foreground permission to us before requesting this launch.
    #[allow(unsafe_code)]
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid);
    }
    // Reap outside the server loop so closing Settings never blocks learning.
    let _ = std::thread::Builder::new()
        .name("retype-settings-watch".into())
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_request_rejects_client_paths_and_arguments() {
        for bytes in [
            r#"{"start_settings":true,"request_id":42,"path":"other.exe"}"#,
            r#"{"start_settings":true,"request_id":42,"args":"--other"}"#,
            r#"{"start_settings":true,"request_id":"42 --other"}"#,
        ] {
            assert!(serde_json::from_str::<Request>(bytes).is_err());
        }
        assert!(
            serde_json::from_str::<Request>(r#"{"start_settings":true,"request_id":42}"#).is_ok()
        );
    }

    #[test]
    fn ordinary_test_process_is_not_an_app_container() {
        assert!(matches!(crate::transport::is_app_container(), Ok(false)));
    }

    #[cfg(feature = "broker")]
    #[test]
    fn helper_rejects_relative_installation_paths() {
        assert_eq!(
            launch(std::path::Path::new("relative"), 42)
                .err()
                .map(|error| error.kind()),
            Some(io::ErrorKind::InvalidInput)
        );
    }
}
