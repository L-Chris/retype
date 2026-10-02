//! TSF input-mode indicator. COM ownership stays in the activating apartment.
use crate::{
    edit,
    tip::{guarded, lock, TipState},
};
use retype_types::{InputEvent, PinyinScheme};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Weak,
};
use windows::Win32::System::Ole::{CONNECT_E_ADVISELIMIT, CONNECT_E_NOCONNECTION};
use windows::Win32::System::Variant::VARIANT;

pub(crate) struct ModeBridge {
    pub compartment: ITfCompartment,
    source: ITfSource,
    cookie: u32,
}
impl ModeBridge {
    pub fn attach(state: &Arc<TipState>, manager: &ITfThreadMgr) -> Result<Self> {
        // SAFETY: Thread-level TSF compartment, accessed only on the owning apartment.
        unsafe {
            let compartments: ITfCompartmentMgr = manager.cast()?;
            let compartment = compartments.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE)?;
            compartment.SetValue(state.tid.load(Ordering::SeqCst), &VARIANT::from(1i32))?;
            let source: ITfSource = compartment.cast()?;
            let sink: ITfCompartmentEventSink = ModeSink {
                state: Arc::downgrade(state),
                compartment: compartment.clone(),
            }
            .into();
            let cookie = source.AdviseSink(&ITfCompartmentEventSink::IID, &sink)?;
            Ok(Self {
                compartment,
                source,
                cookie,
            })
        }
    }
}
impl Drop for ModeBridge {
    fn drop(&mut self) {
        unsafe {
            let _ = self.source.UnadviseSink(self.cookie);
        }
    }
}
#[implement(ITfCompartmentEventSink)]
struct ModeSink {
    state: Weak<TipState>,
    compartment: ITfCompartment,
}
impl ITfCompartmentEventSink_Impl for ModeSink_Impl {
    fn OnChange(&self, _guid: *const GUID) -> Result<()> {
        guarded(|| {
            let Some(state) = self.state.upgrade() else {
                return Ok(());
            };
            if !state.is_activated() {
                return Ok(());
            }
            let Some(session) = state.session() else {
                return Ok(());
            };
            // SAFETY: TSF notifies on this compartment's apartment.
            let chinese = i32::try_from(&unsafe { self.compartment.GetValue()? })? != 0;
            if session.backend.with_kernel(|k| k.is_chinese()) == chinese {
                return Ok(());
            }
            let current = lock(&state.composition).clone();
            if let Some(current) = current {
                edit::request(&state, &current.context, edit::Work::SetChinese(chinese))?;
            } else {
                session.submit(InputEvent::ToggleChinese);
                state.notify_language_bar();
            }
            Ok(())
        })
    }
}
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    UI::{TextServices::*, WindowsAndMessaging::*},
};
use windows_core::{implement, w, Interface, Ref, Result, BOOL, BSTR, GUID};

pub(crate) struct LanguageBar {
    manager: ITfLangBarItemMgr,
    item: ITfLangBarItemButton,
    sink: Arc<Mutex<Option<ITfLangBarItemSink>>>,
}
impl LanguageBar {
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn attach(state: &Arc<TipState>, manager: &ITfThreadMgr) -> Result<Self> {
        let manager: ITfLangBarItemMgr = manager.cast()?;
        let sink = Arc::new(Mutex::new(None));
        let item: ITfLangBarItemButton = Button {
            state: Arc::downgrade(state),
            sink: Arc::clone(&sink),
            hidden: AtomicBool::new(false),
        }
        .into();
        // SAFETY: Item and manager belong to this apartment; Drop removes the registration.
        unsafe {
            manager.AddItem(&item)?;
        }
        Ok(Self {
            manager,
            item,
            sink,
        })
    }
    pub fn sink(&self) -> Option<ITfLangBarItemSink> {
        lock(&self.sink).clone()
    }
}
impl Drop for LanguageBar {
    fn drop(&mut self) {
        // SAFETY: Balances AddItem on this thread. Release sink outside its mutex.
        unsafe {
            let _ = self.manager.RemoveItem(&self.item);
        }
        let sink = lock(&self.sink).take();
        drop(sink);
    }
}

#[implement(ITfLangBarItemButton, ITfSource)]
struct Button {
    state: Weak<TipState>,
    sink: Arc<Mutex<Option<ITfLangBarItemSink>>>,
    hidden: AtomicBool,
}
impl Button_Impl {
    fn mode(&self) -> (bool, PinyinScheme) {
        self.state
            .upgrade()
            .and_then(|s| s.session())
            .map(|s| {
                s.backend
                    .with_kernel(|k| (k.is_chinese(), k.config().pinyin_scheme))
            })
            .unwrap_or((true, PinyinScheme::Full))
    }
    fn update(&self) {
        let sink = lock(&self.sink).clone();
        if let Some(sink) = sink {
            unsafe {
                let _ = sink.OnUpdate(TF_LBI_ICON | TF_LBI_TEXT | TF_LBI_STATUS);
            }
        }
    }
    fn action(&self, work: edit::Work) -> Result<()> {
        guarded(|| {
            let Some(state) = self.state.upgrade() else {
                return Ok(());
            };
            let manager = lock(&state.thread_mgr).clone();
            let context =
                manager.and_then(|m| unsafe { m.GetFocus().and_then(|d| d.GetTop()).ok() });
            if let Some(context) = context {
                edit::request(&state, &context, work)?;
            } else if lock(&state.composition).is_none() {
                if let Some(session) = state.session() {
                    let event = match work {
                        edit::Work::Toggle => InputEvent::ToggleChinese,
                        _ => return Ok(()),
                    };
                    session.submit(event);
                    state.notify_language_bar();
                }
            }
            Ok(())
        })
    }
}
impl ITfLangBarItem_Impl for Button_Impl {
    fn GetInfo(&self, info: *mut TF_LANGBARITEMINFO) -> Result<()> {
        if info.is_null() {
            return Err(E_POINTER.into());
        }
        let mut value = TF_LANGBARITEMINFO {
            clsidService: crate::ids::CLSID_RETYPE_TIP,
            guidItem: GUID_LBI_INPUTMODE,
            dwStyle: TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_BTN_MENU | TF_LBI_STYLE_SHOWNINTRAY,
            ..Default::default()
        };
        for (to, from) in value
            .szDescription
            .iter_mut()
            .zip("retype 输入模式".encode_utf16())
        {
            *to = from;
        }
        // SAFETY: COM supplies one writable TF_LANGBARITEMINFO.
        unsafe {
            *info = value;
        }
        Ok(())
    }
    fn GetStatus(&self) -> Result<u32> {
        Ok(if self.hidden.load(Ordering::SeqCst) {
            TF_LBI_STATUS_HIDDEN
        } else {
            0
        })
    }
    fn Show(&self, show: BOOL) -> Result<()> {
        self.hidden.store(!show.as_bool(), Ordering::SeqCst);
        self.update();
        Ok(())
    }
    fn GetTooltipString(&self) -> Result<BSTR> {
        let (chinese, scheme) = self.mode();
        Ok(BSTR::from(
            format!(
                "retype · {} · {}\n点击或 {} 切换中英；右键打开设置",
                if chinese { "中文" } else { "英文" },
                if scheme == PinyinScheme::Flypy {
                    "小鹤双拼"
                } else {
                    "全拼"
                },
                retype_ai::secrets::shortcuts().mode.label()
            )
            .as_str(),
        ))
    }
}
impl ITfLangBarItemButton_Impl for Button_Impl {
    fn OnClick(&self, click: TfLBIClick, point: &POINT, _rect: *const RECT) -> Result<()> {
        if click == TF_LBI_CLK_LEFT {
            self.action(edit::Work::Toggle)
        } else if click == TF_LBI_CLK_RIGHT {
            guarded(|| {
                // SAFETY: Modal menu belongs to the foreground host; Windows returns a command ID.
                unsafe {
                    let menu = CreatePopupMenu()?;
                    let result = (|| -> Result<u32> {
                        AppendMenuW(menu, MF_STRING, 1, w!("设置"))?;
                        Ok(TrackPopupMenu(
                            menu,
                            TPM_RETURNCMD | TPM_NONOTIFY,
                            point.x,
                            point.y,
                            None,
                            GetForegroundWindow(),
                            None,
                        )
                        .0 as u32)
                    })();
                    let _ = DestroyMenu(menu);
                    let id = result?;
                    if id != 0 {
                        self.OnMenuSelect(id)?;
                    }
                    Ok(())
                }
            })
        } else {
            Ok(())
        }
    }
    fn InitMenu(&self, menu: Ref<'_, ITfMenu>) -> Result<()> {
        // SAFETY: Text slice stays alive during the synchronous call.
        unsafe {
            menu.ok()?.AddMenuItem(
                1,
                0,
                HBITMAP::default(),
                HBITMAP::default(),
                &"设置".encode_utf16().collect::<Vec<_>>(),
                std::ptr::null_mut(),
            )?;
        }
        Ok(())
    }
    fn OnMenuSelect(&self, id: u32) -> Result<()> {
        match id {
            1 => crate::preferences::open_settings(),
            _ => Err(E_INVALIDARG.into()),
        }
    }
    fn GetIcon(&self) -> Result<HICON> {
        guarded(|| mode_icon(self.mode().0))
    }
    fn GetText(&self) -> Result<BSTR> {
        Ok(BSTR::from(if self.mode().0 { "中" } else { "A" }))
    }
}
impl ITfSource_Impl for Button_Impl {
    fn AdviseSink(&self, iid: *const GUID, object: Ref<'_, windows_core::IUnknown>) -> Result<u32> {
        if iid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: COM supplies a readable GUID.
        if unsafe { *iid } != ITfLangBarItemSink::IID {
            return Err(E_NOINTERFACE.into());
        }
        let sink: ITfLangBarItemSink = object.ok()?.cast()?;
        let mut current = lock(&self.sink);
        if current.is_some() {
            return Err(CONNECT_E_ADVISELIMIT.into());
        }
        *current = Some(sink);
        Ok(1)
    }
    fn UnadviseSink(&self, cookie: u32) -> Result<()> {
        if cookie != 1 {
            return Err(CONNECT_E_NOCONNECTION.into());
        }
        let sink = lock(&self.sink).take();
        if sink.is_none() {
            return Err(CONNECT_E_NOCONNECTION.into());
        }
        drop(sink);
        Ok(())
    }
}

fn mode_icon(chinese: bool) -> Result<HICON> {
    // SAFETY: All GDI objects are local; CreateIconIndirect copies the bitmaps.
    // TSF owns the returned HICON and calls DestroyIcon as required by GetIcon.
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON).max(16);
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let color = CreateCompatibleBitmap(screen, size, size);
        // In an AND mask, 1 means transparent. Start with a transparent canvas,
        // then draw only the glyph as opaque pixels.
        let mask_bytes = vec![0xffu8; (((size + 15) / 16) * 2 * size) as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bytes.as_ptr().cast()));
        let mask_dc = CreateCompatibleDC(Some(screen));
        let old_mask = SelectObject(mask_dc, mask.into());
        let old = SelectObject(dc, color.into());
        let brush = CreateSolidBrush(COLORREF(0x00000000));
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: size,
            bottom: size,
        };
        FillRect(dc, &rect, brush);
        let font = CreateFontW(
            -size + 2,
            0,
            0,
            0,
            600,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            0,
            w!("Microsoft YaHei UI"),
        );
        let previous_font = SelectObject(dc, font.into());
        let previous_mask_font = SelectObject(mask_dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        SetBkMode(mask_dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00ffffff));
        SetTextColor(mask_dc, COLORREF(0x00000000));
        let mut glyph: Vec<u16> = (if chinese { "中" } else { "A" }).encode_utf16().collect();
        DrawTextW(
            dc,
            &mut glyph,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        let mut mask_rect = RECT {
            left: 0,
            top: 0,
            right: size,
            bottom: size,
        };
        DrawTextW(
            mask_dc,
            &mut glyph,
            &mut mask_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(dc, previous_font);
        SelectObject(mask_dc, previous_mask_font);
        SelectObject(dc, old);
        SelectObject(mask_dc, old_mask);
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: BOOL(1),
            hbmMask: mask,
            hbmColor: color,
            ..Default::default()
        });
        let _ = DeleteObject(font.into());
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(mask.into());
        let _ = DeleteObject(color.into());
        let _ = DeleteDC(dc);
        let _ = DeleteDC(mask_dc);
        ReleaseDC(None, screen);
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn mode_icons_leave_the_background_transparent() -> Result<()> {
        // SAFETY: Inspect a copy of each icon's AND mask and release all GDI handles.
        unsafe {
            for chinese in [true, false] {
                let icon = mode_icon(chinese)?;
                let mut info = ICONINFO::default();
                GetIconInfo(icon, &mut info)?;
                let screen = GetDC(None);
                let dc = CreateCompatibleDC(Some(screen));
                let old = SelectObject(dc, info.hbmMask.into());
                let size = GetSystemMetrics(SM_CXSMICON).max(16);
                let corner = GetPixel(dc, 0, 0);
                let mut opaque_pixels = 0;
                for y in 0..size {
                    for x in 0..size {
                        if GetPixel(dc, x, y) == COLORREF(0) {
                            opaque_pixels += 1;
                        }
                    }
                }
                SelectObject(dc, old);
                let _ = DeleteDC(dc);
                ReleaseDC(None, screen);
                let _ = DeleteObject(info.hbmMask.into());
                let _ = DeleteObject(info.hbmColor.into());
                DestroyIcon(icon)?;
                assert_eq!(corner, COLORREF(0x00ffffff));
                assert!(opaque_pixels > 0);
            }
        }
        Ok(())
    }

    #[implement(ITfLangBarItemSink)]
    struct Updates(Arc<AtomicU32>);
    impl ITfLangBarItemSink_Impl for Updates_Impl {
        fn OnUpdate(&self, flags: u32) -> Result<()> {
            self.0.fetch_or(flags, Ordering::SeqCst);
            Ok(())
        }
    }
    #[test]
    #[allow(clippy::arc_with_non_send_sync)]
    fn indicator_contract_and_icon_ownership() -> Result<()> {
        let state = TipState::new();
        let session = crate::session::Session::start_with("Z:/missing/retype-test".into());
        *lock(&state.session) = Some(session);
        let item: ITfLangBarItemButton = Button {
            state: Arc::downgrade(&state),
            sink: Arc::new(Mutex::new(None)),
            hidden: AtomicBool::new(false),
        }
        .into();
        // SAFETY: Local COM objects and valid output buffers; every owned HICON is destroyed.
        unsafe {
            let mut info = TF_LANGBARITEMINFO::default();
            item.GetInfo(&mut info)?;
            assert_eq!(info.guidItem, GUID_LBI_INPUTMODE);
            assert_eq!(item.GetText()?.to_string(), "中");
            let icon = item.GetIcon()?;
            assert!(!icon.is_invalid());
            DestroyIcon(icon)?;
            if let Some(session) = state.session() {
                session.submit(InputEvent::ToggleChinese);
            }
            assert_eq!(item.GetText()?.to_string(), "A");
            let icon = item.GetIcon()?;
            assert!(!icon.is_invalid());
            DestroyIcon(icon)?;
            let source: ITfSource = item.cast()?;
            let updates = Arc::new(AtomicU32::new(0));
            let sink: ITfLangBarItemSink = Updates(Arc::clone(&updates)).into();
            let cookie = source.AdviseSink(&ITfLangBarItemSink::IID, &sink)?;
            assert!(source.AdviseSink(&ITfLangBarItemSink::IID, &sink).is_err());
            item.Show(false)?;
            assert_eq!(item.GetStatus()?, TF_LBI_STATUS_HIDDEN);
            assert_ne!(updates.load(Ordering::SeqCst) & TF_LBI_ICON, 0);
            source.UnadviseSink(cookie)?;
            assert!(source.UnadviseSink(cookie).is_err());
        }
        Ok(())
    }

    #[test]
    fn os_open_close_notification_updates_the_engine() -> Result<()> {
        use windows::Win32::System::Com::*;
        // SAFETY: Private STA; only its thread-local TSF compartment is changed.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            let result = (|| -> Result<()> {
                let manager: ITfThreadMgr =
                    CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)?;
                let tid = manager.Activate()?;
                let state = TipState::new();
                state.tid.store(tid, Ordering::SeqCst);
                state.activated.store(true, Ordering::SeqCst);
                let session = crate::session::Session::start_with("Z:/missing/retype-test".into());
                *lock(&state.session) = Some(Arc::clone(&session));
                let bridge = ModeBridge::attach(&state, &manager)?;
                bridge.compartment.SetValue(tid, &VARIANT::from(0i32))?;
                assert!(!session.backend.with_kernel(|k| k.is_chinese()));
                bridge.compartment.SetValue(tid, &VARIANT::from(1i32))?;
                assert!(session.backend.with_kernel(|k| k.is_chinese()));
                drop(bridge);
                manager.Deactivate()?;
                Ok(())
            })();
            CoUninitialize();
            result
        }
    }
}
