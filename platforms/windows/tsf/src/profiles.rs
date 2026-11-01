//! Two TSF profiles share a service and learning store. Activate profiles only
//! after the host edit transaction has returned, on the same owning apartment.
use crate::{
    edit,
    ids::*,
    tip::{guarded, lock, TipState},
};
use retype_types::InputEvent;
use std::sync::{Arc, Weak};
use windows::Win32::{
    Foundation::*,
    System::{Com::*, LibraryLoader::*},
    UI::{TextServices::*, WindowsAndMessaging::*},
};
use windows_core::{implement, w, Interface, Result, BOOL, GUID, PCWSTR};

const SWITCH: u32 = WM_APP + 0x52;
const CLASS: PCWSTR = w!("retype.InputEntry");

pub(crate) fn active_chinese() -> Option<bool> {
    // SAFETY: Called only from the TSF apartment.
    unsafe {
        let manager: ITfInputProcessorProfileMgr =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER).ok()?;
        let mut profile = TF_INPUTPROCESSORPROFILE::default();
        manager
            .GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut profile)
            .ok()?;
        (profile.clsid == CLSID_RETYPE_TIP)
            .then(|| mode(&profile.guidProfile))
            .flatten()
    }
}
fn mode(profile: &GUID) -> Option<bool> {
    if *profile == GUID_PROFILE_RETYPE {
        Some(true)
    } else if *profile == GUID_PROFILE_ENGLISH {
        Some(false)
    } else {
        None
    }
}
pub(crate) fn apply(state: &Arc<TipState>, chinese: bool) -> Result<()> {
    let compartment = lock(&state.mode_bridge)
        .as_ref()
        .map(|bridge| bridge.compartment.clone());
    if let Some(compartment) = compartment {
        // A selected retype entry processes keys in either language.
        unsafe {
            compartment.SetValue(
                state.tid.load(std::sync::atomic::Ordering::SeqCst),
                &windows::Win32::System::Variant::VARIANT::from(1i32),
            )?;
        }
    }
    let Some(session) = state.session() else {
        return Ok(());
    };
    if session.backend.with_kernel(|k| k.is_chinese()) == chinese {
        return Ok(());
    }
    let composition = lock(&state.composition).clone();
    if let Some(composition) = composition {
        edit::request(state, &composition.context, edit::Work::SetChinese(chinese))?;
    } else {
        session.submit(InputEvent::ToggleChinese);
        state.notify_language_bar();
    }
    Ok(())
}
fn select(chinese: bool) -> Result<()> {
    let (language, profile, _) = INPUT_PROFILES[usize::from(!chinese)];
    // SAFETY: No host edit lock or state mutex is held across TSF activation.
    unsafe {
        let profiles: ITfInputProcessorProfiles =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        if !profiles
            .IsEnabledLanguageProfile(&CLSID_RETYPE_TIP, language, &profile)?
            .as_bool()
        {
            return Err(E_FAIL.into());
        }
        let manager: ITfInputProcessorProfileMgr = profiles.cast()?;
        // Stage our profile before changing language so Windows does not pick a
        // different keyboard as the default for the target language.
        manager.ActivateProfile(
            TF_PROFILETYPE_INPUTPROCESSOR,
            language,
            &CLSID_RETYPE_TIP,
            &profile,
            Default::default(),
            TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE,
        )?;
        profiles.ChangeCurrentLanguage(language)?;
        manager.ActivateProfile(
            TF_PROFILETYPE_INPUTPROCESSOR,
            language,
            &CLSID_RETYPE_TIP,
            &profile,
            Default::default(),
            0,
        )?;
    }
    Ok(())
}
pub(crate) fn schedule(state: &Arc<TipState>) {
    let window = lock(&state.entry_bridge)
        .as_ref()
        .map(|bridge| bridge.window);
    if let (Some(window), Some(session)) = (window, state.session()) {
        let chinese = session.backend.with_kernel(|k| k.is_chinese());
        // SAFETY: A message-only window owned by this TSF apartment. Destroying
        // it during deactivation discards queued requests for the old service.
        unsafe {
            let _ = PostMessageW(
                Some(window),
                SWITCH,
                WPARAM(usize::from(chinese)),
                LPARAM(0),
            );
        }
    }
}
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    // SAFETY: The window owns exactly one Box<Weak<TipState>> from creation
    // until WM_NCDESTROY. Upgrade outside state locks before reentrant COM calls.
    unsafe {
        let pointer = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut Weak<TipState>;
        if message == SWITCH && !pointer.is_null() {
            let state = (*pointer).upgrade();
            if let Some(state) = state.filter(|state| state.is_activated()) {
                if let Err(error) = select(wp.0 != 0) {
                    tracing::warn!("retype input entry switch failed: {error}");
                    if let Some(chinese) = active_chinese() {
                        let _ = apply(&state, chinese);
                    }
                }
            }
            return LRESULT(0);
        }
        if message == WM_NCDESTROY && !pointer.is_null() {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            drop(Box::from_raw(pointer));
        }
        DefWindowProcW(window, message, wp, lp)
    }
}
pub(crate) struct EntryBridge {
    source: ITfSource,
    cookie: u32,
    window: HWND,
    instance: HINSTANCE,
}
impl EntryBridge {
    pub fn attach(state: &Arc<TipState>, manager: &ITfThreadMgr) -> Result<Self> {
        // SAFETY: All registration, window and sink operations stay on the STA.
        unsafe {
            let mut module = HMODULE::default();
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                PCWSTR(window_proc as *const () as *const u16),
                &mut module,
            )?;
            let instance = HINSTANCE(module.0);
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: CLASS,
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(windows_core::Error::from_thread());
            }
            let source: ITfSource = manager.cast()?;
            let sink: ITfActiveLanguageProfileNotifySink =
                ProfileSink(Arc::downgrade(state)).into();
            let cookie = source.AdviseSink(&ITfActiveLanguageProfileNotifySink::IID, &sink)?;
            match CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS,
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance),
                None,
            ) {
                Ok(window) => {
                    let pointer = Box::into_raw(Box::new(Arc::downgrade(state)));
                    SetWindowLongPtrW(window, GWLP_USERDATA, pointer as isize);
                    Ok(Self {
                        source,
                        cookie,
                        window,
                        instance,
                    })
                }
                Err(error) => {
                    let _ = source.UnadviseSink(cookie);
                    Err(error)
                }
            }
        }
    }
}
impl Drop for EntryBridge {
    fn drop(&mut self) {
        unsafe {
            let _ = self.source.UnadviseSink(self.cookie);
            let _ = DestroyWindow(self.window);
            // Multiple instances can share the class; the last window unregisters it.
            let _ = UnregisterClassW(CLASS, Some(self.instance));
        }
    }
}
#[implement(ITfActiveLanguageProfileNotifySink)]
struct ProfileSink(Weak<TipState>);
impl ITfActiveLanguageProfileNotifySink_Impl for ProfileSink_Impl {
    fn OnActivated(&self, clsid: *const GUID, profile: *const GUID, active: BOOL) -> Result<()> {
        guarded(|| {
            if !active.as_bool() || clsid.is_null() || profile.is_null() {
                return Ok(());
            }
            // SAFETY: TSF supplies readable GUIDs for this callback.
            let chinese = unsafe {
                if *clsid != CLSID_RETYPE_TIP {
                    return Ok(());
                }
                mode(&*profile)
            };
            if let (Some(state), Some(chinese)) = (self.0.upgrade(), chinese) {
                if state.is_activated() {
                    apply(&state, chinese)?;
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use retype_types::{Key, Modifiers};
    use std::sync::atomic::Ordering;

    #[test]
    fn profile_notifications_select_prediction_language_and_ignore_other_services() -> Result<()> {
        let state = TipState::new();
        state.activated.store(true, Ordering::SeqCst);
        let session = crate::session::Session::start_with("Z:/missing/retype-profile-test".into());
        *lock(&state.session) = Some(Arc::clone(&session));
        let sink: ITfActiveLanguageProfileNotifySink = ProfileSink(Arc::downgrade(&state)).into();
        // SAFETY: Local COM sink; GUID pointers are valid for the entire callback.
        unsafe {
            sink.OnActivated(&CLSID_RETYPE_TIP, &GUID_PROFILE_ENGLISH, true)?;
            assert!(!session.backend.with_kernel(|k| k.is_chinese()));
            session.submit(InputEvent::SetEnglishOptions {
                enabled: true,
                spelling: true,
            });
            for ch in "hel".chars() {
                session.submit(InputEvent::Key {
                    key: Key::Char(ch),
                    mods: Modifiers::NONE,
                    source: retype_types::InputSource::Keyboard,
                });
            }
            assert!(session.backend.with_kernel(|k| k
                .render_state()
                .candidates
                .iter()
                .any(|c| c.text == "hello")));
            session.submit(InputEvent::ResetComposition);
            sink.OnActivated(&GUID::zeroed(), &GUID_PROFILE_RETYPE, true)?;
            sink.OnActivated(&CLSID_RETYPE_TIP, &GUID_PROFILE_RETYPE, false)?;
            assert!(!session.backend.with_kernel(|k| k.is_chinese()));
            sink.OnActivated(&CLSID_RETYPE_TIP, &GUID_PROFILE_RETYPE, true)?;
            assert!(session.backend.with_kernel(|k| k.is_chinese()));
            state.activated.store(false, Ordering::SeqCst);
            sink.OnActivated(&CLSID_RETYPE_TIP, &GUID_PROFILE_ENGLISH, true)?;
            assert!(session.backend.with_kernel(|k| k.is_chinese()));
        }
        Ok(())
    }
}
