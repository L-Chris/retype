//! COM 类工厂。TSF 通过 `DllGetClassObject(CLSID_RETYPE_TIP, IID_IClassFactory)`
//! 拿到它，再 `CreateInstance` 出 TIP 对象。

use crate::tip::RetypeTip;
use core::ffi::c_void;
use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_POINTER};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows_core::{implement, IUnknown, Interface, Ref, Result, BOOL, GUID};

#[implement(IClassFactory)]
pub struct ClassFactory;

impl IClassFactory_Impl for ClassFactory_Impl {
    // 签名由 COM vtable 决定：裸指针出参是 QueryInterface 的固有形态，
    // 我们无法把它标成 unsafe fn（trait 不允许），只能在实现处显式豁免并自行保证：
    // 失败路径一律把出参置空，成功路径写入的是 AddRef 过的有效指针。
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn CreateInstance(
        &self,
        punkouter: Ref<'_, IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        if !ppvobject.is_null() {
            // 失败时必须把出参置空，否则调用方可能读到一个野指针
            unsafe { *ppvobject = core::ptr::null_mut() };
        }
        if ppvobject.is_null() || riid.is_null() {
            return Err(windows_core::Error::from(E_POINTER));
        }
        // TSF TIP 不支持聚合（系统也从不聚合它）
        if !punkouter.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }

        let tip = RetypeTip::create();
        let unknown: IUnknown = tip.cast()?;
        // QueryInterface 会 AddRef；`unknown` 离开作用域时 Release，
        // 净效果是调用方拿到一个引用计数为 1 的接口指针
        let hr = unsafe { unknown.query(riid, ppvobject) };
        if hr.is_ok() {
            Ok(())
        } else {
            unsafe { *ppvobject = core::ptr::null_mut() };
            Err(hr.into())
        }
    }

    fn LockServer(&self, _flock: BOOL) -> Result<()> {
        // 不做引用计数：`DllCanUnloadNow` 恒返回 S_FALSE，
        // DLL 在宿主进程生命周期内常驻。对 TIP 来说这比「卸载/重载」更安全 ——
        // 卸载时如果还有 sink 挂在宿主的 thread mgr 上，行为是未定义的。
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use windows::Win32::Foundation::E_NOINTERFACE;
    use windows::Win32::UI::TextServices::{ITfKeystrokeMgr, ITfTextInputProcessorEx};

    #[test]
    fn create_instance_yields_a_text_input_processor() {
        let factory: IClassFactory = ClassFactory.into();
        // SAFETY: 纯进程内调用，不涉及跨公寓编组
        let tip: ITfTextInputProcessorEx = unsafe { factory.CreateInstance(None) }.unwrap();
        // 能 QI 到 IUnknown 就说明 vtable 与引用计数生成正确
        let _: IUnknown = tip.cast().unwrap();
    }

    #[test]
    fn unimplemented_interface_returns_e_nointerface() {
        let factory: IClassFactory = ClassFactory.into();
        // 我们没实现 ITfKeystrokeMgr，QI 必须干净地失败并把出参置空
        let e = unsafe { factory.CreateInstance::<_, ITfKeystrokeMgr>(None) }.unwrap_err();
        assert_eq!(e.code(), E_NOINTERFACE);
    }

    #[test]
    fn aggregation_is_rejected() {
        let factory: IClassFactory = ClassFactory.into();
        let outer: IUnknown = ClassFactory.into();
        let e = unsafe { factory.CreateInstance::<_, ITfTextInputProcessorEx>(Some(&outer)) }
            .unwrap_err();
        assert_eq!(e.code(), CLASS_E_NOAGGREGATION);
    }

    #[test]
    fn each_instance_has_its_own_state() {
        // 同一进程里可能同时存在多个 TIP 实例（多语言配置档），
        // 它们绝不能共享组字状态
        let factory: IClassFactory = ClassFactory.into();
        let a: ITfTextInputProcessorEx = unsafe { factory.CreateInstance(None) }.unwrap();
        let b: ITfTextInputProcessorEx = unsafe { factory.CreateInstance(None) }.unwrap();
        assert_ne!(a.as_raw(), b.as_raw());
    }

    #[test]
    fn class_factory_survives_repeated_creation() {
        let factory: IClassFactory = ClassFactory.into();
        for _ in 0..16 {
            let tip: ITfTextInputProcessorEx = unsafe { factory.CreateInstance(None) }.unwrap();
            drop(tip);
        }
    }
}
