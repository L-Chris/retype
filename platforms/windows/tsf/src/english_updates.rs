//! Deliver spelling updates on the host STA, including when no prefix matched.
use crate::{
    edit,
    tip::{lock, TipState},
};
use retype_engine::KernelBackend;
use retype_types::KernelAction;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, Weak},
};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{KillTimer, SetTimer},
};

thread_local! {
    static WATCHERS: RefCell<HashMap<usize, Weak<TipState>>> = RefCell::new(HashMap::new());
}

pub(crate) fn watch(state: &Arc<TipState>) {
    WATCHERS.with(|watchers| {
        let mut watchers = watchers.borrow_mut();
        if watchers
            .values()
            .any(|weak| weak.ptr_eq(&Arc::downgrade(state)))
        {
            return;
        }
        // SAFETY: A thread-local timer runs only on the calling TSF apartment.
        let id = unsafe { SetTimer(None, 0, 30, Some(tick)) };
        if id != 0 {
            watchers.insert(id, Arc::downgrade(state));
        }
    });
}

unsafe extern "system" fn tick(_: HWND, _: u32, id: usize, _: u32) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let state = WATCHERS.with(|watchers| watchers.borrow().get(&id).and_then(Weak::upgrade));
        let mut refresh = false;
        let keep = state.as_ref().is_some_and(|state| {
            if !state.is_activated() {
                return false;
            }
            let Some(session) = state.session() else {
                return false;
            };
            if !session
                .backend
                .with_kernel(|k| !k.is_chinese() && k.has_composition())
            {
                return false;
            }
            if state.pending.load(std::sync::atomic::Ordering::SeqCst) != 0 {
                return true;
            }
            while let Some(action) = session.backend.poll_action() {
                refresh |= matches!(action, KernelAction::Render(_));
            }
            !refresh
        });
        if !keep {
            // SAFETY: This ID belongs to this thread's timer registry.
            let _ = unsafe { KillTimer(None, id) };
            WATCHERS.with(|watchers| watchers.borrow_mut().remove(&id));
        }
        if refresh {
            if let Some(state) = state {
                let composition = lock(&state.composition).clone();
                if let Some(composition) = composition {
                    let _ = edit::request(&state, &composition.context, edit::Work::Refresh);
                }
            }
        }
    }));
}
