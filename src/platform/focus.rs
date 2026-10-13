//! Do Not Disturb = Windows' "Quiet Hours" profile (Focus Assist / Win11
//! DND). There's no public API; the shell's own quick settings use the
//! `IQuietHoursSettings` COM interface, as do third-party toggles.
//!   Unrestricted  -> notifications on
//!   PriorityOnly  -> Do not disturb (priority notifications only)

use std::ffi::c_void;

use windows::{
    Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance, CoTaskMemFree},
    core::{GUID, HRESULT, IUnknown, Interface, PCWSTR, PWSTR},
};

const CLSID_QUIET_HOURS_SETTINGS: GUID = GUID::from_u128(0xf53321fa_34f8_4b7f_b9a3_361877cb94cf);
const IID_IQUIET_HOURS_SETTINGS: GUID = GUID::from_u128(0x6bff4732_81ec_4ffb_ae67_b6c1bc29631f);
pub const PROFILE_OFF: &str = "Microsoft.QuietHoursProfile.Unrestricted";
pub const PROFILE_DND: &str = "Microsoft.QuietHoursProfile.PriorityOnly";

/// Raw interface pointer (vtable: IUnknown(3), get_UserSelectedProfile=3, put_UserSelectedProfile=4).
struct QuietHours(*mut c_void);

impl QuietHours {
    fn new() -> Option<Self> {
        unsafe {
            let unk: IUnknown = CoCreateInstance(&CLSID_QUIET_HOURS_SETTINGS, None, CLSCTX_LOCAL_SERVER).ok()?;
            let mut raw = std::ptr::null_mut();
            unk.query(&IID_IQUIET_HOURS_SETTINGS, &mut raw).ok().ok()?;
            (!raw.is_null()).then_some(Self(raw))
        }
    }
    unsafe fn method(&self, index: usize) -> *const c_void {
        unsafe { *(*(self.0 as *const *const *const c_void)).add(index) }
    }
}

impl Drop for QuietHours {
    fn drop(&mut self) {
        unsafe {
            let _ = IUnknown::from_raw(self.0);
        }
    }
}

/// Current profile id, e.g. "Microsoft.QuietHoursProfile.Unrestricted".
pub fn profile() -> Option<String> {
    type Get = unsafe extern "system" fn(*mut c_void, *mut PWSTR) -> HRESULT;
    let qh = QuietHours::new()?;
    unsafe {
        let get: Get = std::mem::transmute(qh.method(3));
        let mut out = PWSTR::null();
        get(qh.0, &mut out).ok().ok()?;
        let s = out.to_string().ok();
        CoTaskMemFree(Some(out.0 as *const c_void));
        s
    }
}

pub fn dnd_enabled() -> Option<bool> {
    profile().map(|p| p != PROFILE_OFF)
}

pub fn set_dnd(on: bool) -> bool {
    type Put = unsafe extern "system" fn(*mut c_void, PCWSTR) -> HRESULT;
    let Some(qh) = QuietHours::new() else { return false };
    let id: Vec<u16> = (if on { PROFILE_DND } else { PROFILE_OFF }).encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let put: Put = std::mem::transmute(qh.method(4));
        put(qh.0, PCWSTR(id.as_ptr())).is_ok()
    }
}
