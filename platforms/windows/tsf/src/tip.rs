//! TIP 与各类 sink 的 COM 实现。
//!
//! **M0 的安全边界**：这个 DLL 可以被系统加载、可以激活、可以收到按键，
//! 但 `OnKeyDown` / `OnTestKeyDown` 一律返回「不吃这个键」。
//! 因为组字串读写（`ITfEditSession`）是 M1 的工作 —— 在能正确上屏之前吃掉按键，
//! 等于把用户的字吞了。宁可先做一个「装了但什么都不干」的输入法。
//!
//! 想观察内核在真实宿主进程里的行为，用影子模式：
//! ```powershell
//! $env:RETYPE_TSF_SHADOW = "1"
//! ```
//! 影子模式会把按键喂给内核（并写日志），但依然返回不吃键。

use crate::keymap;
use crate::session::Session;
use retype_types::{InputEvent, InputSource, KernelAction, Key};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::TextServices::{
    ITfContext, ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfTextInputProcessorEx,
    ITfTextInputProcessorEx_Impl, ITfTextInputProcessor_Impl, ITfThreadMgr,
};
use windows_core::{implement, Interface, Ref, Result, BOOL};

/// 一次激活期间共享的状态。
///
/// TIP 对象和各个 sink 是**独立的 COM 对象**，通过 `Arc<TipState>` 共享。
/// 这样就不需要在 sink 回调里从 `_Impl` 反向 cast 回外层接口
/// （windows 0.62 的 `IUnknownImpl` 没有提供便捷的自 cast）。
pub struct TipState {
    pub tid: AtomicU32,
    pub activated: AtomicBool,
    /// TSF 的接口不是 `Send`/`Sync`，但 TIP 与其 sink 始终在同一个 STA 线程上被调用，
    /// `Mutex` 在这里只是提供内部可变性，不是跨线程同步。
    pub thread_mgr: Mutex<Option<ITfThreadMgr>>,
    pub session: Mutex<Option<Arc<Session>>>,
}

impl TipState {
    /// TIP 与其 sink 始终运行在同一个 STA 线程上：`Arc` 只用于共享所有权，
    /// 不跨线程传递，所以 `TipState` 不需要 `Send`/`Sync`
    /// （`ITfThreadMgr` 在 windows 0.62 里本来也不是 `Send`/`Sync`）。
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            tid: AtomicU32::new(0),
            activated: AtomicBool::new(false),
            thread_mgr: Mutex::new(None),
            session: Mutex::new(None),
        })
    }

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        match m.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    pub fn activate(
        self: &Arc<Self>,
        ptim: Ref<'_, ITfThreadMgr>,
        tid: u32,
        flags: u32,
    ) -> Result<()> {
        self.tid.store(tid, Ordering::SeqCst);
        if let Ok(mgr) = ptim.ok() {
            // clone 只是 AddRef；接口不 Send，所以只能在当前线程存着
            *Self::lock(&self.thread_mgr) = Some(mgr.clone());
        }

        // 词库加载在后台线程，这里必须立刻返回（P1）
        let session = Session::start();
        tracing::info!(
            "retype TIP 激活: tid={tid} flags={flags:#x} dict={:?} shadow={}",
            session.dict_path,
            session.shadow
        );
        *Self::lock(&self.session) = Some(Arc::clone(&session));

        // 按键 sink 挂在 ITfKeystrokeMgr 上（ITfThreadMgr 通过 QI 暴露它）
        let mgr_opt = Self::lock(&self.thread_mgr).clone();
        if let Some(mgr) = mgr_opt {
            let ks: ITfKeystrokeMgr = mgr.cast()?;
            let sink: ITfKeyEventSink = KeyEventSink {
                state: Arc::clone(self),
            }
            .into();
            // SAFETY: ks 是本次激活期间有效的接口指针。
            // windows 0.62 的 `Param<T, InterfaceType>` 只接受**借用**，
            // 传值会报 trait bound 不满足。
            unsafe {
                if let Err(e) = ks.AdviseKeyEventSink(tid, &sink, true) {
                    tracing::error!("AdviseKeyEventSink 失败: {e:?}");
                    return Err(e);
                }
            }
            // sink 在这里被 TSF 持有（AdviseKeyEventSink 内部 AddRef），
            // 我们的这份引用可以正常释放
            drop(sink);
        }

        self.activated.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub fn deactivate(&self) -> Result<()> {
        let tid = self.tid.load(Ordering::SeqCst);

        // 未定稿的组字串必须取消，否则宿主应用里会留下「僵尸下划线」
        if let Some(session) = Self::lock(&self.session).take() {
            let _ = session.submit(InputEvent::Key {
                key: Key::Escape,
                mods: retype_types::Modifiers::NONE,
                source: InputSource::Keyboard,
            });
        }

        let mgr = Self::lock(&self.thread_mgr).take();
        if let Some(mgr) = mgr {
            if let Ok(ks) = mgr.cast::<ITfKeystrokeMgr>() {
                // SAFETY: 同 activate
                unsafe {
                    if let Err(e) = ks.UnadviseKeyEventSink(tid) {
                        // 反注册失败不该阻止 deactivate 完成，否则宿主会认为我们还活着
                        tracing::warn!("UnadviseKeyEventSink 失败: {e:?}");
                    }
                }
            }
        }

        self.activated.store(false, Ordering::SeqCst);
        tracing::info!("retype TIP 已停用: tid={tid}");
        Ok(())
    }

    pub fn is_activated(&self) -> bool {
        self.activated.load(Ordering::SeqCst)
    }

    pub fn session(&self) -> Option<Arc<Session>> {
        Self::lock(&self.session).clone()
    }
}

// ─────────────────────────── TIP 入口 ───────────────────────────

#[implement(ITfTextInputProcessorEx)]
pub struct RetypeTip {
    pub state: Arc<TipState>,
}

impl RetypeTip {
    /// 直接产出 COM 接口而不是 `Self`：调用方只关心接口，
    /// 内部状态通过 `Arc<TipState>` 与各 sink 共享。
    pub fn create() -> ITfTextInputProcessorEx {
        Self {
            state: TipState::new(),
        }
        .into()
    }
}

impl ITfTextInputProcessor_Impl for RetypeTip_Impl {
    fn Activate(&self, ptim: Ref<'_, ITfThreadMgr>, tid: u32) -> Result<()> {
        self.state.activate(ptim, tid, 0)
    }

    fn Deactivate(&self) -> Result<()> {
        self.state.deactivate()
    }
}

impl ITfTextInputProcessorEx_Impl for RetypeTip_Impl {
    fn ActivateEx(&self, ptim: Ref<'_, ITfThreadMgr>, tid: u32, dwflags: u32) -> Result<()> {
        self.state.activate(ptim, tid, dwflags)
    }
}

// ─────────────────────────── 按键 sink ───────────────────────────

#[implement(ITfKeyEventSink)]
pub struct KeyEventSink {
    pub state: Arc<TipState>,
}

impl KeyEventSink {
    /// 把一次 Win32 按键翻译成内核事件并投递。
    ///
    /// 返回 `true` 表示「这个键该归输入法」。M0 恒定返回 `false`，
    /// 见模块级注释里的安全边界说明。
    fn handle_key(&self, wparam: WPARAM, lparam: LPARAM, is_test: bool) -> bool {
        let Some(session) = self.state.session() else {
            return false;
        };
        // M0：只有影子模式才喂内核，且任何情况下都不吃键
        if !session.shadow {
            return false;
        }
        if is_test {
            // OnTestKeyDown 会被高频调用（每个按键至少一次），不做实际处理，
            // 只回答「吃不吃」。真正的翻译留给 OnKeyDown。
            return false;
        }

        let vk = (wparam.0 & 0xFFFF) as u16;
        let Some(key) = keymap::translate_vk(vk) else {
            return false;
        };
        let mods = keymap::read_modifiers();
        let acts = session.submit(InputEvent::Key {
            key,
            mods,
            source: InputSource::Keyboard,
        });
        for a in &acts {
            match a {
                KernelAction::Render(r) => {
                    tracing::debug!(
                        "shadow: 组字={:?} 候选={:?}",
                        r.composition,
                        r.candidates
                            .iter()
                            .take(5)
                            .map(|c| c.text.as_str())
                            .collect::<Vec<_>>()
                    );
                }
                KernelAction::Commit(c) => {
                    // M1 起这里要开一个 ITfEditSession 把文字写进宿主
                    tracing::info!("shadow: 内核要求上屏 {c:?}（M0 未实现 edit session，已忽略）");
                }
                KernelAction::PassThrough => {}
                KernelAction::Side(s) => tracing::debug!("shadow: 副作用 {s:?}"),
            }
        }
        let _ = lparam;
        false
    }
}

impl ITfKeyEventSink_Impl for KeyEventSink_Impl {
    fn OnSetFocus(&self, _fforeground: BOOL) -> Result<()> {
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        _pic: Ref<'_, ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<BOOL> {
        Ok(BOOL::from(self.handle_key(wparam, lparam, true)))
    }

    fn OnTestKeyUp(
        &self,
        _pic: Ref<'_, ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        Ok(BOOL::from(false))
    }

    fn OnKeyDown(&self, _pic: Ref<'_, ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        // TIP 跑在宿主进程里：这里 panic 就是宿主崩溃，必须兜住
        let eaten = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.handle_key(wparam, lparam, false)
        }))
        .unwrap_or_else(|_| {
            tracing::error!("OnKeyDown panic，已吞掉并按「不吃键」处理");
            false
        });
        Ok(BOOL::from(eaten))
    }

    fn OnKeyUp(&self, _pic: Ref<'_, ITfContext>, _wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        Ok(BOOL::from(false))
    }

    fn OnPreservedKey(
        &self,
        _pic: Ref<'_, ITfContext>,
        _rguid: *const windows_core::GUID,
    ) -> Result<BOOL> {
        Ok(BOOL::from(false))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn tip_state_starts_inactive() {
        let s = TipState::new();
        assert!(!s.is_activated());
        assert!(s.session().is_none());
        assert_eq!(s.tid.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn deactivate_without_activate_is_harmless() {
        // 宿主有可能在任何时刻调用 Deactivate，不能因此 panic
        let s = TipState::new();
        assert!(s.deactivate().is_ok());
        assert!(!s.is_activated());
    }

    #[test]
    fn key_sink_without_session_does_not_eat_keys() {
        let s = TipState::new();
        let sink = KeyEventSink { state: s };
        // wparam = 'A' (0x41)
        assert!(!sink.handle_key(WPARAM(0x41), LPARAM(0), false));
        assert!(!sink.handle_key(WPARAM(0x41), LPARAM(0), true));
    }

    #[test]
    fn bool_conversion_matches_eaten_semantics() {
        assert!(!BOOL::from(false).as_bool());
        assert!(BOOL::from(true).as_bool());
    }
}
