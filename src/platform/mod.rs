//! Thin, safe wrappers over the Win32/WinRT APIs the shell needs.
pub mod audio;
pub mod autostart;
pub mod blur;
pub mod bluetooth;
pub mod desktops;
pub mod focus;
pub mod ghosts;
pub mod shell_items;
pub mod system;
pub mod window;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED, CoInitializeEx};

/// Initialises COM for a worker thread (shell APIs want STA, WinRT is fine with MTA).
pub fn com_init(sta: bool) {
    unsafe {
        let _ = CoInitializeEx(None, if sta { COINIT_APARTMENTTHREADED } else { COINIT_MULTITHREADED });
    }
}
