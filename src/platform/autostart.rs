//! "Start with Windows" (`windows.autostart`): a per-user Run entry pointing
//! at the running executable. Disabling it in Task Manager's Startup tab is
//! respected (Windows keeps that flag elsewhere, we never touch it).

use std::path::Path;

use windows::{
    Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW},
    core::HSTRING,
};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const NAME: &str = "Caelestia";

/// Dev builds (cargo's `target` dir) and named test instances never register.
fn eligible(exe: &Path) -> bool {
    std::env::var_os("CAELESTIA_INSTANCE").is_none() && !exe.components().any(|c| c.as_os_str().eq_ignore_ascii_case("target"))
}

/// Adds, updates (the exe moved) or removes the Run entry to match `enabled`.
pub fn sync(enabled: bool) {
    let Ok(exe) = std::env::current_exe() else { return };
    if !eligible(&exe) {
        return;
    }
    let want = format!("\"{}\"", exe.display());
    let have = super::system::reg_string(HKEY_CURRENT_USER, RUN, NAME);
    let (key, name) = (HSTRING::from(RUN), HSTRING::from(NAME));
    unsafe {
        if enabled && have.as_deref() != Some(want.as_str()) {
            let data: Vec<u16> = want.encode_utf16().chain(Some(0)).collect();
            let r = RegSetKeyValueW(HKEY_CURRENT_USER, &key, &name, REG_SZ.0, Some(data.as_ptr().cast()), (data.len() * 2) as u32);
            match r.ok() {
                Ok(()) => log::info!("autostart enabled: {want}"),
                Err(e) => log::warn!("autostart: {e}"),
            }
        } else if !enabled && have.is_some() {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, &key, &name);
            log::info!("autostart disabled");
        }
    }
}
