//! Host-rendered composition underline.
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{E_INVALIDARG, E_POINTER};
use windows::Win32::UI::TextServices::*;
use windows_core::{implement, Result, BSTR, GUID, HRESULT};

pub const ATTRIBUTE: GUID = GUID::from_u128(0x70e34804_2913_4359_923e_74b45b7f12db);
pub fn info() -> ITfDisplayAttributeInfo {
    Attribute.into()
}
pub fn enumeration() -> IEnumTfDisplayAttributeInfo {
    Attributes {
        consumed: AtomicBool::new(false),
    }
    .into()
}
#[implement(ITfDisplayAttributeInfo)]
struct Attribute;
impl ITfDisplayAttributeInfo_Impl for Attribute_Impl {
    fn GetGUID(&self) -> Result<GUID> {
        Ok(ATTRIBUTE)
    }
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("retype composition"))
    }
    fn GetAttributeInfo(&self, value: *mut TF_DISPLAYATTRIBUTE) -> Result<()> {
        if value.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: Caller provided a writable attribute out-parameter.
        unsafe {
            *value = TF_DISPLAYATTRIBUTE {
                lsStyle: TF_LS_DOT,
                bAttr: TF_ATTR_INPUT,
                ..Default::default()
            };
        }
        Ok(())
    }
    fn SetAttributeInfo(&self, value: *const TF_DISPLAYATTRIBUTE) -> Result<()> {
        if value.is_null() {
            return Err(E_POINTER.into());
        }
        Err(E_INVALIDARG.into())
    }
    fn Reset(&self) -> Result<()> {
        Ok(())
    }
}
#[implement(IEnumTfDisplayAttributeInfo)]
struct Attributes {
    consumed: AtomicBool,
}
impl IEnumTfDisplayAttributeInfo_Impl for Attributes_Impl {
    fn Clone(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(Attributes {
            consumed: AtomicBool::new(self.consumed.load(Ordering::Relaxed)),
        }
        .into())
    }
    fn Next(
        &self,
        count: u32,
        out: *mut Option<ITfDisplayAttributeInfo>,
        fetched: *mut u32,
    ) -> Result<()> {
        if (count > 0 && out.is_null()) || (count != 1 && fetched.is_null()) {
            return Err(E_POINTER.into());
        }
        let n = u32::from(count > 0 && !self.consumed.swap(true, Ordering::Relaxed));
        // SAFETY: Validated output pointers; at most one owned interface is written.
        unsafe {
            if !fetched.is_null() {
                *fetched = n;
            }
            if n == 1 {
                out.write(Some(info()));
            }
        }
        if n < count {
            Err(HRESULT(1).into())
        } else {
            Ok(())
        }
    }
    fn Reset(&self) -> Result<()> {
        self.consumed.store(false, Ordering::Relaxed);
        Ok(())
    }
    fn Skip(&self, count: u32) -> Result<()> {
        if count == 0 {
            return Ok(());
        }
        let was = self.consumed.swap(true, Ordering::Relaxed);
        if was || count > 1 {
            Err(HRESULT(1).into())
        } else {
            Ok(())
        }
    }
}
