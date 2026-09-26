//! Real TSF integration tests with an in-memory ITextStoreACP host.
#![allow(
    unused_variables,
    clippy::not_unsafe_ptr_arg_deref,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arc_with_non_send_sync
)]
use super::*;
use crate::tip::{lock, TipState};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;

use windows_core::{implement, Interface, Result, HRESULT};
#[derive(Default)]
struct Data {
    text: Vec<u16>,
    start: i32,
    end: i32,
    sink: Option<ITextStoreACPSink>,
    reject: bool,
    defer: bool,
    deferred_flags: Option<u32>,
}
#[implement(ITextStoreACP)]
struct Store {
    data: Arc<Mutex<Data>>,
}
impl ITextStoreACP_Impl for Store_Impl {
    fn AdviseSink(
        &self,
        riid: *const windows_core::GUID,
        punk: windows_core::Ref<windows_core::IUnknown>,
        dwmask: u32,
    ) -> windows_core::Result<()> {
        lock(&self.data).sink = Some(punk.ok()?.cast()?);
        Ok(())
    }
    fn UnadviseSink(
        &self,
        punk: windows_core::Ref<windows_core::IUnknown>,
    ) -> windows_core::Result<()> {
        lock(&self.data).sink = None;
        Ok(())
    }
    fn RequestLock(&self, dwlockflags: u32) -> windows_core::Result<windows_core::HRESULT> {
        if lock(&self.data).reject {
            return Ok(E_ACCESSDENIED);
        }
        if lock(&self.data).defer {
            lock(&self.data).deferred_flags = Some(dwlockflags);
            return Ok(TS_S_ASYNC);
        }
        let sink = lock(&self.data).sink.clone().ok_or(E_FAIL)?;
        unsafe {
            sink.OnLockGranted(TEXT_STORE_LOCK_FLAGS(dwlockflags))?;
        }
        Ok(HRESULT(0))
    }
    fn GetStatus(&self) -> windows_core::Result<TS_STATUS> {
        Ok(TS_STATUS {
            dwDynamicFlags: 0,
            dwStaticFlags: TS_SS_NOHIDDENTEXT,
        })
    }
    fn QueryInsert(
        &self,
        acpteststart: i32,
        acptestend: i32,
        cch: u32,
        pacpresultstart: *mut i32,
        pacpresultend: *mut i32,
    ) -> windows_core::Result<()> {
        unsafe {
            *pacpresultstart = acpteststart;
            *pacpresultend = acptestend;
        }
        Ok(())
    }
    fn GetSelection(
        &self,
        ulindex: u32,
        ulcount: u32,
        pselection: *mut TS_SELECTION_ACP,
        pcfetched: *mut u32,
    ) -> windows_core::Result<()> {
        let d = lock(&self.data);
        unsafe {
            *pselection = TS_SELECTION_ACP {
                acpStart: d.start,
                acpEnd: d.end,
                style: TS_SELECTIONSTYLE {
                    ase: TS_AE_END,
                    fInterimChar: BOOL(0),
                },
            };
            *pcfetched = 1;
        }
        Ok(())
    }
    fn SetSelection(
        &self,
        ulcount: u32,
        pselection: *const TS_SELECTION_ACP,
    ) -> windows_core::Result<()> {
        let mut d = lock(&self.data);
        unsafe {
            d.start = (*pselection).acpStart;
            d.end = (*pselection).acpEnd;
        }
        Ok(())
    }
    fn GetText(
        &self,
        acpstart: i32,
        acpend: i32,
        pchplain: windows_core::PWSTR,
        cchplainreq: u32,
        pcchplainret: *mut u32,
        prgruninfo: *mut TS_RUNINFO,
        cruninforeq: u32,
        pcruninforet: *mut u32,
        pacpnext: *mut i32,
    ) -> windows_core::Result<()> {
        let d = lock(&self.data);
        let end = if acpend < 0 {
            d.text.len()
        } else {
            acpend as usize
        };
        let start = acpstart as usize;
        if start > end || end > d.text.len() {
            return Err(E_INVALIDARG.into());
        }
        let n = (end - start).min(cchplainreq as usize);
        unsafe {
            if n > 0 {
                std::ptr::copy_nonoverlapping(d.text[start..].as_ptr(), pchplain.0, n);
            }
            if !pcchplainret.is_null() {
                *pcchplainret = n as u32;
            }
            if !pacpnext.is_null() {
                *pacpnext = (start + n) as i32;
            }
            if !pcruninforet.is_null() {
                *pcruninforet = if cruninforeq > 0 { 1 } else { 0 };
            }
            if cruninforeq > 0 {
                *prgruninfo = TS_RUNINFO {
                    uCount: n as u32,
                    r#type: TS_RT_PLAIN,
                };
            }
        }
        Ok(())
    }
    fn SetText(
        &self,
        dwflags: u32,
        acpstart: i32,
        acpend: i32,
        pchtext: &windows_core::PCWSTR,
        cch: u32,
    ) -> windows_core::Result<TS_TEXTCHANGE> {
        let mut d = lock(&self.data);
        let text = if cch == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(pchtext.0, cch as usize) }
        };
        d.text
            .splice(acpstart as usize..acpend as usize, text.iter().copied());
        d.start = acpstart + cch as i32;
        d.end = d.start;
        Ok(TS_TEXTCHANGE {
            acpStart: acpstart,
            acpOldEnd: acpend,
            acpNewEnd: d.end,
        })
    }
    fn GetFormattedText(
        &self,
        acpstart: i32,
        acpend: i32,
    ) -> windows_core::Result<windows::Win32::System::Com::IDataObject> {
        Err(E_NOTIMPL.into())
    }
    fn GetEmbedded(
        &self,
        acppos: i32,
        rguidservice: *const windows_core::GUID,
        riid: *const windows_core::GUID,
    ) -> windows_core::Result<windows_core::IUnknown> {
        Err(E_NOTIMPL.into())
    }
    fn QueryInsertEmbedded(
        &self,
        pguidservice: *const windows_core::GUID,
        pformatetc: *const windows::Win32::System::Com::FORMATETC,
    ) -> windows_core::Result<windows_core::BOOL> {
        Ok(false.into())
    }
    fn InsertEmbedded(
        &self,
        dwflags: u32,
        acpstart: i32,
        acpend: i32,
        pdataobject: windows_core::Ref<windows::Win32::System::Com::IDataObject>,
    ) -> windows_core::Result<TS_TEXTCHANGE> {
        Err(E_NOTIMPL.into())
    }
    fn InsertTextAtSelection(
        &self,
        dwflags: u32,
        pchtext: &windows_core::PCWSTR,
        cch: u32,
        pacpstart: *mut i32,
        pacpend: *mut i32,
        pchange: *mut TS_TEXTCHANGE,
    ) -> windows_core::Result<()> {
        let (start, end) = {
            let d = lock(&self.data);
            (d.start, d.end)
        };
        unsafe {
            if !pacpstart.is_null() {
                *pacpstart = start;
            }
            if !pacpend.is_null() {
                *pacpend = end;
            }
        }
        if dwflags & TS_IAS_QUERYONLY != 0 {
            return Ok(());
        }
        let change = self.SetText(0, start, end, pchtext, cch)?;
        unsafe {
            if !pchange.is_null() {
                *pchange = change;
            }
        }
        Ok(())
    }
    fn InsertEmbeddedAtSelection(
        &self,
        dwflags: u32,
        pdataobject: windows_core::Ref<windows::Win32::System::Com::IDataObject>,
        pacpstart: *mut i32,
        pacpend: *mut i32,
        pchange: *mut TS_TEXTCHANGE,
    ) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn RequestSupportedAttrs(
        &self,
        dwflags: u32,
        cfilterattrs: u32,
        pafilterattrs: *const windows_core::GUID,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn RequestAttrsAtPosition(
        &self,
        acppos: i32,
        cfilterattrs: u32,
        pafilterattrs: *const windows_core::GUID,
        dwflags: u32,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn RequestAttrsTransitioningAtPosition(
        &self,
        acppos: i32,
        cfilterattrs: u32,
        pafilterattrs: *const windows_core::GUID,
        dwflags: u32,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn FindNextAttrTransition(
        &self,
        acpstart: i32,
        acphalt: i32,
        cfilterattrs: u32,
        pafilterattrs: *const windows_core::GUID,
        dwflags: u32,
        pacpnext: *mut i32,
        pffound: *mut windows_core::BOOL,
        plfoundoffset: *mut i32,
    ) -> windows_core::Result<()> {
        unsafe {
            *pacpnext = acphalt;
            *pffound = BOOL(0);
            *plfoundoffset = 0;
        }
        Ok(())
    }
    fn RetrieveRequestedAttrs(
        &self,
        ulcount: u32,
        paattrvals: *mut TS_ATTRVAL,
        pcfetched: *mut u32,
    ) -> windows_core::Result<()> {
        unsafe {
            *pcfetched = 0;
        }
        Ok(())
    }
    fn GetEndACP(&self) -> windows_core::Result<i32> {
        Ok(lock(&self.data).text.len() as i32)
    }
    fn GetActiveView(&self) -> windows_core::Result<u32> {
        Ok(0)
    }
    fn GetACPFromPoint(
        &self,
        vcview: u32,
        ptscreen: *const windows::Win32::Foundation::POINT,
        dwflags: u32,
    ) -> windows_core::Result<i32> {
        Err(E_NOTIMPL.into())
    }
    fn GetTextExt(
        &self,
        vcview: u32,
        acpstart: i32,
        acpend: i32,
        prc: *mut windows::Win32::Foundation::RECT,
        pfclipped: *mut windows_core::BOOL,
    ) -> windows_core::Result<()> {
        unsafe {
            *prc = RECT {
                left: 10 + acpstart * 8,
                top: 10,
                right: 10 + acpend * 8,
                bottom: 30,
            };
            *pfclipped = BOOL(0);
        }
        Ok(())
    }
    fn GetScreenExt(&self, vcview: u32) -> windows_core::Result<windows::Win32::Foundation::RECT> {
        Ok(RECT {
            left: 0,
            top: 0,
            right: 500,
            bottom: 500,
        })
    }
    fn GetWnd(&self, vcview: u32) -> windows_core::Result<windows::Win32::Foundation::HWND> {
        Ok(HWND::default())
    }
}
#[test]
fn real_tsf_composition_commit_cancel_and_passthrough() -> Result<()> {
    // SAFETY: Isolated COM apartment and text store; no system registration or desktop input changes.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let result = run_host();
        CoUninitialize();
        result
    }
}
fn run_host() -> Result<()> {
    unsafe {
        let manager: ITfThreadMgr =
            CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)?;
        let tid = manager.Activate()?;
        let document = manager.CreateDocumentMgr()?;
        let data = Arc::new(Mutex::new(Data::default()));
        let store: ITextStoreACP = Store {
            data: Arc::clone(&data),
        }
        .into();
        let mut context = None;
        let mut cookie = 0;
        document.CreateContext(tid, 0, &store, &mut context, &mut cookie)?;
        let context = context.ok_or(E_FAIL)?;
        document.Push(&context)?;
        manager.SetFocus(&document)?;
        let state = TipState::new();
        state.tid.store(tid, Ordering::SeqCst);
        state.activated.store(true, Ordering::SeqCst);
        *lock(&state.thread_mgr) = Some(manager.clone());
        // Known deterministic dictionary; no background loader races.
        let session =
            crate::session::Session::start_with(std::path::PathBuf::from("Z:/retype-test/no-dict"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !session.dict_ready() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let (dict, _) = retype_dict::from_pairs([
            ("你好", "ni hao", 10000.0),
            ("你", "ni", 100.0),
            ("好", "hao", 100.0),
        ]);
        session.dict.install(Arc::new(dict));
        session.submit(InputEvent::SetPinyinScheme(
            retype_types::PinyinScheme::Full,
        ));
        *lock(&state.session) = Some(session);
        *lock(&state.window) = None;
        for ch in "nihao".chars() {
            request(&state, &context, Work::Key(Key::Char(ch), Modifiers::NONE))?;
        }
        assert_eq!(String::from_utf16_lossy(&lock(&data).text), "nihao");
        assert!(lock(&state.composition).is_some());
        request(&state, &context, Work::Key(Key::Space, Modifiers::NONE))?;
        assert_eq!(String::from_utf16_lossy(&lock(&data).text), "你好");
        assert!(lock(&state.composition).is_none());
        // Deferred writes must not touch the engine/document before the host grants its lock.
        lock(&data).defer = true;
        request(&state, &context, Work::Key(Key::Char('n'), Modifiers::NONE))?;
        assert_eq!(String::from_utf16_lossy(&lock(&data).text), "你好");
        assert!(!state
            .session()
            .unwrap()
            .backend
            .with_kernel(|k| k.has_composition()));
        let flags = lock(&data).deferred_flags.take().expect("deferred lock");
        lock(&data).defer = false;
        let sink = lock(&data).sink.clone().expect("advised sink");
        sink.OnLockGranted(TEXT_STORE_LOCK_FLAGS(flags))?;
        assert_eq!(String::from_utf16_lossy(&lock(&data).text), "你好n");
        request(&state, &context, Work::Key(Key::Escape, Modifiers::NONE))?;
        assert_eq!(state.pending.load(Ordering::SeqCst), 0);
        // Focus loss invalidates a deferred key before it can edit the old document.
        lock(&data).defer = true;
        request(&state, &context, Work::Key(Key::Char('n'), Modifiers::NONE))?;
        state.epoch.fetch_add(1, Ordering::SeqCst);
        let flags = lock(&data).deferred_flags.take().expect("deferred lock");
        lock(&data).defer = false;
        sink.OnLockGranted(TEXT_STORE_LOCK_FLAGS(flags))?;
        assert_eq!(String::from_utf16_lossy(&lock(&data).text), "你好");
        assert_eq!(state.pending.load(Ordering::SeqCst), 0);
        // A rejected edit request must leave both the document and engine untouched.
        lock(&data).reject = true;
        assert!(request(&state, &context, Work::Key(Key::Char('a'), Modifiers::NONE)).is_err());
        assert_eq!(state.pending.load(Ordering::SeqCst), 0);
        assert!(!state
            .session()
            .unwrap()
            .backend
            .with_kernel(|k| k.has_composition()));
        lock(&data).reject = false;
        for ch in "ni".chars() {
            request(&state, &context, Work::Key(Key::Char(ch), Modifiers::NONE))?;
        }
        request(&state, &context, Work::Key(Key::Escape, Modifiers::NONE))?;
        assert_eq!(String::from_utf16_lossy(&lock(&data).text), "你好");
        for ch in "nihao".chars() {
            request(&state, &context, Work::Key(Key::Char(ch), Modifiers::NONE))?;
        }
        request(&state, &context, Work::Key(Key::Char('.'), Modifiers::NONE))?;
        assert_eq!(String::from_utf16_lossy(&lock(&data).text), "你好你好.");
        for ch in "ni".chars() {
            request(&state, &context, Work::Key(Key::Char(ch), Modifiers::NONE))?;
        }
        request(&state, &context, Work::Key(Key::Char('0'), Modifiers::NONE))?;
        assert_eq!(String::from_utf16_lossy(&lock(&data).text), "你好你好.ni0");
        assert!(lock(&state.composition).is_none());
        let session = state.session().expect("active session");
        session.submit(InputEvent::SetPinyinScheme(
            retype_types::PinyinScheme::Flypy,
        ));
        for ch in "nihc".chars() {
            request(&state, &context, Work::Key(Key::Char(ch), Modifiers::NONE))?;
        }
        let generation = session.backend.with_kernel(|k| k.generation());
        request(
            &state,
            &context,
            Work::Choose(0, generation.wrapping_sub(1)),
        )?;
        assert_eq!(
            String::from_utf16_lossy(&lock(&data).text),
            "你好你好.ni0nihc"
        );
        request(&state, &context, Work::Choose(0, generation))?;
        assert_eq!(
            String::from_utf16_lossy(&lock(&data).text),
            "你好你好.ni0你好"
        );
        request(&state, &context, Work::Toggle)?;
        assert!(!session.backend.with_kernel(|k| k.is_chinese()));
        request(&state, &context, Work::Toggle)?;
        assert!(session.backend.with_kernel(|k| k.is_chinese()));
        state.deactivate()?;
        document.Pop(TF_POPF_ALL)?;
        manager.Deactivate()?;
        Ok(())
    }
}
