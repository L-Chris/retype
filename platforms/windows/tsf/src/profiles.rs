//! System-selected entries determine English prediction; Shift only changes the
//! internal Chinese/ASCII mode and never selects a different system entry.
use crate::{
    edit,
    ids::*,
    tip::{guarded, lock, TipState},
};
use retype_types::InputEvent;
use std::sync::{Arc, Weak};
use windows::Win32::{System::Com::*, UI::TextServices::*};
use windows_core::{implement, Interface, Result, BOOL, GUID};

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
    session.submit(InputEvent::SetEnglishPrediction(!chinese));
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
pub(crate) struct EntryBridge {
    source: ITfSource,
    cookie: u32,
}
impl EntryBridge {
    pub fn attach(state: &Arc<TipState>, manager: &ITfThreadMgr) -> Result<Self> {
        // SAFETY: Profile notifications stay on this owning TSF apartment.
        unsafe {
            let source: ITfSource = manager.cast()?;
            let sink: ITfActiveLanguageProfileNotifySink =
                ProfileSink(Arc::downgrade(state)).into();
            let cookie = source.AdviseSink(&ITfActiveLanguageProfileNotifySink::IID, &sink)?;
            Ok(Self { source, cookie })
        }
    }
}
impl Drop for EntryBridge {
    fn drop(&mut self) {
        unsafe {
            let _ = self.source.UnadviseSink(self.cookie);
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
