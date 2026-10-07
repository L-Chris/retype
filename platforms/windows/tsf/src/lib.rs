//! retype Windows 输入法 —— TSF Text Input Processor。
//!
//! 产出 `retype_ime.dll`（cdylib），由系统注入到每一个需要文本输入的进程。
//!
//! ## 本 crate 的定位
//!
//! 只做 **TSF 管线**：注册、激活、按键翻译、组字串读写、候选窗定位。
//! 所有「智能」都在 `retype-engine` 及以下的跨平台内核里，
//! 这里一行输入逻辑都不该有 —— 这是 Android 端能复用整个内核的前提。
//!
//! ## 唯一的 unsafe 边界
//!
//! 整个仓库只有这个 crate 允许 `unsafe`（COM/Win32 FFI）。
//! 每一处都带 `// SAFETY:` 注释说明为什么成立。
//! 红线见 docs/windows-tsf.md：TIP 跑在宿主进程里，我们崩溃就是宿主崩溃。
#![allow(unsafe_code)]

mod candidate;
pub mod class_factory;
mod display;
mod edit;
mod english_updates;
pub mod ids;
pub mod keymap;
mod langbar;
mod packs;
mod popup;
mod preferences;
mod registration;
mod search;
pub mod session;
#[path = "../../common/settings_log.rs"]
mod settings_log;
mod stats;
pub mod tip;
mod translation;
#[cfg(windows)]
mod voice;

use class_factory::ClassFactory;
use core::ffi::c_void;
use ids::CLSID_RETYPE_TIP;
use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_POINTER, E_UNEXPECTED};
use windows::Win32::System::Com::IClassFactory;
use windows_core::{IUnknown, Interface, GUID, HRESULT};

const S_OK_HR: HRESULT = HRESULT(0);
/// 常驻不卸载：卸载时若还有 sink 挂在宿主的 thread mgr 上，行为未定义。
const S_FALSE_HR: HRESULT = HRESULT(1);

/// 系统通过它拿到类工厂。
///
/// # Safety
/// 由 COM 运行时调用，参数必须是有效的接口指针出参。
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() {
        return E_POINTER;
    }
    // 失败路径必须把出参置空：调用方（ctfmon/宿主）会无条件读它
    *ppv = core::ptr::null_mut();
    if rclsid.is_null() || riid.is_null() {
        return E_POINTER;
    }
    if *rclsid != CLSID_RETYPE_TIP {
        return CLASS_E_CLASSNOTAVAILABLE;
    }

    // panic 不能穿过 FFI 边界（Rust panic 跨 extern "system" 是 UB）
    let made = std::panic::catch_unwind(|| {
        let factory: IClassFactory = ClassFactory.into();
        factory.cast::<IUnknown>()
    });
    match made {
        Ok(Ok(unknown)) => unknown.query(riid, ppv),
        Ok(Err(e)) => e.code(),
        Err(_) => E_UNEXPECTED,
    }
}

/// 注册 COM 类、TSF 中文配置和键盘类别，需要管理员权限。
/// 安装器和开发脚本共用此入口；失败返回实际 HRESULT。
///
/// # Safety
/// 由 COM 运行时调用。
#[no_mangle]
pub unsafe extern "system" fn DllRegisterServer() -> HRESULT {
    registration_result(registration::register)
}

/// # Safety
/// 由 COM 运行时调用。
#[no_mangle]
pub unsafe extern "system" fn DllUnregisterServer() -> HRESULT {
    registration_result(registration::unregister)
}

fn registration_result(action: fn() -> windows_core::Result<()>) -> HRESULT {
    match std::panic::catch_unwind(action) {
        Ok(Ok(())) => S_OK_HR,
        Ok(Err(error)) => error.code(),
        Err(_) => E_UNEXPECTED,
    }
}

/// # Safety
/// 由 COM 运行时调用。
#[no_mangle]
pub unsafe extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE_HR
}

/// 不依赖 TSF 的自检：装配一次会话，喂一串按键，确认组字和单字降级候选正常。
///
/// 给安装器和 CI 用 —— 「DLL 能被加载」和「装进去真能打字」是两件事，
/// 这个函数仅验证内核链路，不验证系统注册或宿主中文上屏。
pub fn selftest() -> Result<String, String> {
    let state = tip::TipState::new();
    // 用一个不存在的词库路径，顺带验证降级路径（§7：词库缺失 → 单字模式）
    let session = session::Session::start_with(std::path::PathBuf::from(
        "Z:/retype-selftest/does-not-exist.tsv",
    ));
    let backend = &session.backend;

    use retype_engine::KernelBackend;
    use retype_types::{InputEvent, InputSource, Key, Modifiers};

    let mut last = String::new();
    for c in "nihaomashijie".chars() {
        let acts = backend.submit(InputEvent::Key {
            key: Key::Char(c),
            mods: Modifiers::NONE,
            source: InputSource::Keyboard,
        });
        for a in acts {
            if let retype_types::KernelAction::Render(r) = a {
                last = r.composition;
            }
        }
    }
    if last != "nihaomashijie" {
        return Err(format!("组字串不对，期望 nihaomashijie，实际 {last:?}"));
    }
    // 词库异步加载期间原样字母不会显示为候选；等单字降级表装入后再验中文候选。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !session.dict_ready() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if !session.dict_ready() {
        return Err("单字降级词库未能加载".into());
    }
    backend.submit(InputEvent::Key {
        key: Key::Escape,
        mods: Modifiers::NONE,
        source: InputSource::Keyboard,
    });
    for c in "ni".chars() {
        backend.submit(InputEvent::Key {
            key: Key::Char(c),
            mods: Modifiers::NONE,
            source: InputSource::Keyboard,
        });
    }
    let n = backend.with_kernel(|k| k.candidate_texts().len());
    if n == 0 {
        return Err("单字降级词库未给出 ni 的中文候选".into());
    }
    // 停用一次，确认 deactivate 路径不会炸
    state
        .deactivate()
        .map_err(|e| format!("deactivate 失败: {e:?}"))?;

    Ok(format!(
        "自检通过：组字 {last}，候选 {n} 条，词库已加载={}",
        session.dict_ready()
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn registration_errors_cross_ffi_as_hresult() {
        assert_eq!(registration_result(|| Ok(())), S_OK_HR);
        assert_eq!(registration_result(|| Err(E_POINTER.into())), E_POINTER);
        assert_eq!(
            registration_result(|| panic!("registration panic")),
            E_UNEXPECTED
        );
    }

    #[test]
    fn selftest_passes_with_a_missing_dict() {
        match selftest() {
            Ok(msg) => assert!(msg.contains("自检通过"), "{msg}"),
            Err(e) => panic!("selftest 失败: {e}"),
        }
    }

    #[test]
    fn dll_get_class_object_rejects_unknown_clsid() {
        let bogus = GUID::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = unsafe { DllGetClassObject(&bogus, &IUnknown::IID, &mut out) };
        assert_eq!(hr, CLASS_E_CLASSNOTAVAILABLE);
        assert!(out.is_null(), "失败时必须把出参置空");
    }

    #[test]
    fn dll_get_class_object_rejects_null_out_param() {
        let hr =
            unsafe { DllGetClassObject(&CLSID_RETYPE_TIP, &IUnknown::IID, core::ptr::null_mut()) };
        assert_eq!(hr, E_POINTER);
    }

    #[test]
    fn dll_get_class_object_returns_a_factory() {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = unsafe { DllGetClassObject(&CLSID_RETYPE_TIP, &IClassFactory::IID, &mut out) };
        assert_eq!(hr, S_OK_HR);
        assert!(!out.is_null());
        // SAFETY: out 是刚按 IID_IClassFactory 返回的接口指针，用完 Release
        let _factory: IClassFactory = unsafe { IClassFactory::from_raw(out) };
    }

    #[test]
    fn dll_never_unloads() {
        // TIP 卸载时机极难判断，常驻是刻意的选择
        assert_eq!(unsafe { DllCanUnloadNow() }, S_FALSE_HR);
    }
}
