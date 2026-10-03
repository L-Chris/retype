//! Machine-wide registration, used only by setup/regsvr32.
use crate::ids::*;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, HMODULE, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::TextServices::*;
use windows_core::{w, Error, Result, PCWSTR};

// The installed DLL and dictionary live under Program Files so packaged hosts can
// load them. Keep registration and removal symmetric across both registry views.
const CATEGORIES: &[windows_core::GUID] = &[
    GUID_TFCAT_TIP_KEYBOARD,
    GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
    GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
    GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
];

struct ComApartment(bool);
impl ComApartment {
    fn new() -> Result<Self> {
        // SAFETY: Initializes COM on this thread; an existing apartment is also usable.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            return Ok(Self(false));
        }
        hr.ok()?;
        Ok(Self(true))
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: Balances this thread's successful CoInitializeEx, including S_FALSE.
            unsafe { CoUninitialize() };
        }
    }
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: Owns a successfully opened registry key.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}
fn class_key() -> String {
    format!("SOFTWARE\\Classes\\CLSID\\{CLSID_RETYPE_TIP_STR}")
}

fn module_path() -> Result<Vec<u16>> {
    let mut module = HMODULE::default();
    let mut path = vec![0u16; 32768];
    // SAFETY: Address belongs to this DLL; buffers remain valid throughout the calls.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(module_path as *const () as *const u16),
            &mut module,
        )?;
        let len = GetModuleFileNameW(Some(module), &mut path) as usize;
        if len == 0 || len >= path.len() {
            return Err(Error::from_thread());
        }
        path.truncate(len);
    }
    Ok(path)
}
fn write_string(key: &Key, name: PCWSTR, value: &[u16]) -> Result<()> {
    let bytes: Vec<u8> = value
        .iter()
        .copied()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    // SAFETY: Valid key and NUL-terminated UTF-16 bytes, copied synchronously by Windows.
    unsafe { RegSetValueExW(key.0, name, None, REG_SZ, Some(&bytes)).ok() }
}
pub fn register() -> Result<()> {
    let _apartment = ComApartment::new()?;
    let path = module_path()?;
    // Keep NUL terminators in backing storage even though the API takes explicit
    // lengths. Some TSF registration paths read the buffers as PCWSTR and can
    // otherwise persist adjacent heap data in the profile description.
    let description: Vec<u16> = DISPLAY_NAME.encode_utf16().chain([0]).collect();
    let icon_path: Vec<u16> = path.iter().copied().chain([0]).collect();
    let key_path: Vec<u16> = format!("{}\\InprocServer32\0", class_key())
        .encode_utf16()
        .collect();
    // SAFETY: Valid output key and strings; COM interfaces stay on this thread.
    unsafe {
        let profiles: ITfInputProcessorProfiles =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        let categories: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        let mut key = HKEY::default();
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key_path.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let key = Key(key);
        write_string(&key, PCWSTR::null(), &path)?;
        write_string(
            &key,
            w!("ThreadingModel"),
            &"Apartment".encode_utf16().collect::<Vec<_>>(),
        )?;
        profiles.Register(&CLSID_RETYPE_TIP)?;
        profiles.AddLanguageProfile(
            &CLSID_RETYPE_TIP,
            LANGID_ZH_CN,
            &GUID_PROFILE_RETYPE,
            &description[..description.len() - 1],
            &icon_path[..icon_path.len() - 1],
            0,
        )?;
        for category in CATEGORIES {
            categories.RegisterCategory(&CLSID_RETYPE_TIP, category, &CLSID_RETYPE_TIP)?;
        }
        // user-profile.ps1 both enrolls the keyboard with InstallLayoutOrTip and
        // enables it for the original desktop user with EnableLanguageProfile.
        // Neither operation alone guarantees a listed, switchable keyboard.
        profiles.EnableLanguageProfileByDefault(
            &CLSID_RETYPE_TIP,
            LANGID_ZH_CN,
            &GUID_PROFILE_RETYPE,
            false,
        )?;
    }
    Ok(())
}
pub fn unregister() -> Result<()> {
    let _apartment = ComApartment::new()?;
    // Attempt every cleanup even if an earlier step fails, then report the first error.
    // SAFETY: COM objects and registry buffers remain valid on this thread.
    unsafe {
        let profiles: ITfInputProcessorProfiles =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        let categories: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        let mut category = Ok(());
        for id in CATEGORIES {
            let result = categories.UnregisterCategory(&CLSID_RETYPE_TIP, id, &CLSID_RETYPE_TIP);
            category = category.and(result);
        }
        let profile = profiles.Unregister(&CLSID_RETYPE_TIP);
        let path: Vec<u16> = class_key().encode_utf16().chain([0]).collect();
        let status = RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(path.as_ptr()));
        let registry = if status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            status.ok()
        };
        category.and(profile).and(registry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires installed and user-enabled retype; affects only this test process"]
    fn installed_tip_can_activate_in_test_process() -> Result<()> {
        use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
        };
        let _apartment = ComApartment::new()?;
        struct ActiveThread(ITfThreadMgr);
        impl Drop for ActiveThread {
            fn drop(&mut self) {
                // SAFETY: This guard stays on the thread it activated.
                let _ = unsafe { self.0.Deactivate() };
            }
        }
        // SAFETY: All COM interfaces are local to this initialized thread. FORPROCESS
        // leaves the user's desktop/session selection unchanged.
        unsafe {
            // Profile activation needs an active TSF host on the calling thread.
            // A console test process does not get one from Windows automatically.
            let thread: ITfThreadMgr =
                CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)?;
            thread.Activate()?;
            let _active_thread = ActiveThread(thread.clone());
            let document = thread.CreateDocumentMgr()?;
            thread.SetFocus(&document)?;
            let manager: ITfInputProcessorProfileMgr =
                CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
            let profiles: ITfInputProcessorProfiles =
                CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
            assert!(
                profiles
                    .IsEnabledLanguageProfile(
                        &CLSID_RETYPE_TIP,
                        LANGID_ZH_CN,
                        &GUID_PROFILE_RETYPE
                    )?
                    .as_bool(),
                "installed retype profile must be enabled before activation"
            );
            let _tip: ITfTextInputProcessorEx =
                CoCreateInstance(&CLSID_RETYPE_TIP, None, CLSCTX_INPROC_SERVER)
                    .inspect_err(|error| eprintln!("loading installed TIP failed: {error}"))?;
            struct InputLanguage(ITfInputProcessorProfiles, u16);
            impl Drop for InputLanguage {
                fn drop(&mut self) {
                    // SAFETY: Restore this process's input language on its STA.
                    let _ = unsafe { self.0.ChangeCurrentLanguage(self.1) };
                }
            }
            let _language = InputLanguage(profiles.clone(), profiles.GetCurrentLanguage()?);
            // CI starts with an English input language. Enabling a Chinese
            // profile alone does not switch the host's input language to it.
            profiles.ChangeCurrentLanguage(LANGID_ZH_CN)?;
            assert_eq!(profiles.GetCurrentLanguage()?, LANGID_ZH_CN);
            manager
                .ActivateProfile(
                    TF_PROFILETYPE_INPUTPROCESSOR,
                    LANGID_ZH_CN,
                    &CLSID_RETYPE_TIP,
                    &GUID_PROFILE_RETYPE,
                    HKL::default(),
                    TF_IPPMF_FORPROCESS,
                )
                .inspect_err(|error| eprintln!("activating installed TIP failed: {error}"))?;
            let mut active = TF_INPUTPROCESSORPROFILE::default();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            let result = loop {
                if let Err(error) = manager.GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut active)
                {
                    break Err(error);
                }
                if (active.clsid == CLSID_RETYPE_TIP && active.guidProfile == GUID_PROFILE_RETYPE)
                    || std::time::Instant::now() >= deadline
                {
                    break Ok(());
                }
                // Deliver TSF notifications on this STA instead of assuming that
                // an asynchronous profile change is already visible.
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            let cleanup = manager.DeactivateProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                LANGID_ZH_CN,
                &CLSID_RETYPE_TIP,
                &GUID_PROFILE_RETYPE,
                HKL::default(),
                TF_IPPMF_FORPROCESS,
            );
            result?;
            cleanup?;
            assert_eq!(active.clsid, CLSID_RETYPE_TIP);
            assert_eq!(active.guidProfile, GUID_PROFILE_RETYPE);
        }
        Ok(())
    }

    /// Read-only check; run after installing the newly built DLL on a clean Windows VM.
    #[test]
    #[ignore = "requires a registered retype DLL; run on the installer test VM"]
    fn installed_tip_is_discoverable_and_loadable() -> Result<()> {
        let _apartment = ComApartment::new()?;
        // SAFETY: COM is initialized; all interfaces and buffers remain on this thread.
        unsafe {
            let categories: ITfCategoryMgr =
                CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
            for (category, purpose) in [
                (GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT, "packaged Windows hosts"),
                (GUID_TFCAT_TIPCAP_UIELEMENTENABLED, "UI-less hosts"),
            ] {
                let items = categories.EnumItemsInCategory(&category)?;
                let mut found = false;
                loop {
                    let mut ids = [windows_core::GUID::zeroed()];
                    let mut fetched = 0;
                    items.Next(&mut ids, Some(&mut fetched)).ok()?;
                    if fetched == 0 {
                        break;
                    }
                    if ids[0] == CLSID_RETYPE_TIP {
                        found = true;
                        break;
                    }
                }
                assert!(found, "installed retype must support {purpose}");
            }
            let profiles: ITfInputProcessorProfiles =
                CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
            let description = profiles.GetLanguageProfileDescription(
                &CLSID_RETYPE_TIP,
                LANGID_ZH_CN,
                &GUID_PROFILE_RETYPE,
            )?;
            assert_eq!(
                description.to_string(),
                DISPLAY_NAME,
                "registered profile name must not contain trailing heap data"
            );
            assert!(profiles
                .IsEnabledLanguageProfile(&CLSID_RETYPE_TIP, LANGID_ZH_CN, &GUID_PROFILE_RETYPE)?
                .as_bool());
            let enumeration = profiles.EnumLanguageProfiles(LANGID_ZH_CN)?;
            let mut found = false;
            loop {
                let mut entries = [TF_LANGUAGEPROFILE::default()];
                let mut fetched = 0;
                enumeration.Next(&mut entries, &mut fetched)?;
                if fetched == 0 {
                    break;
                }
                if entries[0].clsid == CLSID_RETYPE_TIP
                    && entries[0].guidProfile == GUID_PROFILE_RETYPE
                {
                    assert_eq!(entries[0].catid, GUID_TFCAT_TIP_KEYBOARD);
                    found = true;
                    break;
                }
            }
            assert!(found, "retype must appear among Chinese keyboard profiles");
            let _tip: ITfTextInputProcessorEx =
                CoCreateInstance(&CLSID_RETYPE_TIP, None, CLSCTX_INPROC_SERVER)?;
        }
        Ok(())
    }
}
