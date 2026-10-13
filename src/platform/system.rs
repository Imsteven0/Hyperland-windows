//! System actions and information: launching, power/session, wallpaper,
//! keep-awake, battery and the About page facts.

use std::{os::windows::process::CommandExt, path::Path, process::Command};

use windows::{
    Win32::{
        Foundation::HWND,
        System::{
            Power::{ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, GetSystemPowerStatus, SYSTEM_POWER_STATUS, SetSuspendState, SetThreadExecutionState},
            Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW},
            Shutdown::{EWX_LOGOFF, EXIT_WINDOWS_FLAGS, ExitWindowsEx, LockWorkStation, SHUTDOWN_REASON},
        },
        UI::{
            Shell::ShellExecuteW,
            WindowsAndMessaging::{
                SPI_GETDESKWALLPAPER, SPI_SETDESKWALLPAPER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SW_SHOWNORMAL, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
                SystemParametersInfoW,
            },
        },
    },
    core::{HSTRING, PCWSTR},
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Opens a file, folder, URL or `ms-settings:` URI with its default handler.
pub fn open(target: &str) {
    unsafe {
        let _ = ShellExecuteW(Some(HWND::default()), &HSTRING::from("open"), &HSTRING::from(target), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Runs a command line (first element is the program) detached.
pub fn spawn(cmd: &[String]) {
    let Some((prog, args)) = cmd.split_first() else { return };
    if prog.contains(':') && !prog.contains('\\') && args.is_empty() {
        return open(prog); // URIs like ms-settings:sound
    }
    if let Err(e) = Command::new(prog).args(args).spawn() {
        log::warn!("failed to spawn {prog}: {e}");
        open(prog);
    }
}

pub fn reveal(path: &Path) {
    let _ = Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
}

pub fn keep_awake(on: bool) {
    unsafe {
        let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS };
        SetThreadExecutionState(flags);
    }
}

pub fn session(action: &str) {
    match action {
        "lock" => unsafe {
            let _ = LockWorkStation();
        },
        "logout" => unsafe {
            let _ = ExitWindowsEx(EXIT_WINDOWS_FLAGS(EWX_LOGOFF.0), SHUTDOWN_REASON(0));
        },
        "sleep" => unsafe {
            let _ = SetSuspendState(false, false, false);
        },
        // shutdown.exe takes care of the SE_SHUTDOWN privilege dance for us.
        "shutdown" => {
            let _ = Command::new("shutdown").args(["/s", "/t", "0"]).creation_flags(CREATE_NO_WINDOW).spawn();
        }
        "reboot" => {
            let _ = Command::new("shutdown").args(["/r", "/t", "0"]).creation_flags(CREATE_NO_WINDOW).spawn();
        }
        other => log::warn!("unknown session action {other}"),
    }
}

pub fn current_wallpaper() -> Option<String> {
    let mut buf = [0u16; 1024];
    unsafe {
        SystemParametersInfoW(SPI_GETDESKWALLPAPER, buf.len() as u32, Some(buf.as_mut_ptr().cast()), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).ok()?;
    }
    let s = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]);
    (!s.is_empty()).then_some(s)
}

pub fn set_wallpaper(path: &Path) -> bool {
    let wide: Vec<u16> = path.as_os_str().encode_wide_nul();
    unsafe { SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, Some(wide.as_ptr() as *mut _), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE).is_ok() }
}

/// Sets the wallpaper on a worker thread: SPI_SETDESKWALLPAPER transcodes the
/// image and broadcasts WM_SETTINGCHANGE, which can take a second. Requests
/// queued meanwhile collapse to the newest (fast browsing in the launcher).
pub fn set_wallpaper_async(path: std::path::PathBuf) {
    use std::sync::OnceLock;
    static TX: OnceLock<crossbeam_channel::Sender<std::path::PathBuf>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded::<std::path::PathBuf>();
        std::thread::Builder::new()
            .name("wallpaper".into())
            .spawn(move || {
                while let Ok(mut path) = rx.recv() {
                    if let Some(newest) = rx.try_iter().last() {
                        path = newest;
                    }
                    if !set_wallpaper(&path) {
                        log::warn!("could not set wallpaper {}", path.display());
                    }
                }
            })
            .expect("spawn wallpaper thread");
        tx
    });
    let _ = tx.send(path);
}

trait EncodeWideNul {
    fn encode_wide_nul(&self) -> Vec<u16>;
}
impl EncodeWideNul for std::ffi::OsStr {
    fn encode_wide_nul(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Battery {
    pub present: bool,
    pub percent: f32,
    pub charging: bool,
}

pub fn battery() -> Battery {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() {
        return Battery::default();
    }
    // BatteryFlag 128 = no system battery, 255 = unknown.
    let present = s.BatteryFlag & 128 == 0 && s.BatteryFlag != 255 && s.BatteryLifePercent <= 100;
    Battery { present, percent: s.BatteryLifePercent.min(100) as f32 / 100.0, charging: s.ACLineStatus == 1 }
}

// ---------------- registry helpers ----------------

pub fn reg_string(root: HKEY, key: &str, value: &str) -> Option<String> {
    let mut buf = vec![0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(root, &HSTRING::from(key), &HSTRING::from(value), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size)).ok().ok()?;
    }
    let len = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..len]).trim().to_owned())
}

pub fn reg_dword(root: HKEY, key: &str, value: &str) -> Option<u32> {
    let mut v = 0u32;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(root, &HSTRING::from(key), &HSTRING::from(value), RRF_RT_REG_DWORD, None, Some((&mut v as *mut u32).cast()), Some(&mut size)).ok().ok()?;
    }
    Some(v)
}

pub fn reg_binary(root: HKEY, key: &str, value: &str) -> Option<Vec<u8>> {
    let mut size = 0u32;
    unsafe {
        RegGetValueW(root, &HSTRING::from(key), &HSTRING::from(value), RRF_RT_REG_BINARY, None, None, Some(&mut size)).ok().ok()?;
        let mut buf = vec![0u8; size as usize];
        RegGetValueW(root, &HSTRING::from(key), &HSTRING::from(value), RRF_RT_REG_BINARY, None, Some(buf.as_mut_ptr().cast()), Some(&mut size)).ok().ok()?;
        buf.truncate(size as usize);
        Some(buf)
    }
}

pub const HKCU: HKEY = HKEY_CURRENT_USER;

#[derive(Debug, Clone, Default)]
pub struct AboutInfo {
    pub hostname: String,
    pub device: String,
    pub os: String,
    pub build: String,
    pub firmware: String,
    pub user: String,
}

pub fn about() -> AboutInfo {
    const NT: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    const BIOS: &str = r"HARDWARE\DESCRIPTION\System\BIOS";
    let build_no: u32 = reg_string(HKEY_LOCAL_MACHINE, NT, "CurrentBuild").and_then(|b| b.parse().ok()).unwrap_or(0);
    let mut product = reg_string(HKEY_LOCAL_MACHINE, NT, "ProductName").unwrap_or_else(|| "Windows".into());
    // Windows 11 still reports "Windows 10" in ProductName.
    if build_no >= 22000 {
        product = product.replace("Windows 10", "Windows 11");
    }
    let display = reg_string(HKEY_LOCAL_MACHINE, NT, "DisplayVersion").unwrap_or_default();
    let ubr = reg_dword(HKEY_LOCAL_MACHINE, NT, "UBR").unwrap_or(0);
    AboutInfo {
        hostname: std::env::var("COMPUTERNAME").unwrap_or_default(),
        device: reg_string(HKEY_LOCAL_MACHINE, BIOS, "SystemManufacturer").unwrap_or_default(),
        os: if display.is_empty() { product } else { format!("{product} {display}") },
        build: format!("{build_no}.{ubr}"),
        firmware: reg_string(HKEY_LOCAL_MACHINE, BIOS, "BIOSVersion").unwrap_or_default(),
        user: std::env::var("USERNAME").unwrap_or_default(),
    }
}

/// Moves a file to the Recycle Bin (never a hard delete).
pub fn recycle(path: &Path) -> bool {
    use windows::Win32::UI::Shell::{FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW};
    let from: Vec<u16> = path.as_os_str().encode_wide_nul().into_iter().chain(std::iter::once(0)).collect(); // double-NUL list
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    unsafe { SHFileOperationW(&mut op) == 0 }
}

/// Seconds since boot.
pub fn uptime_secs() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() / 1000 }
}
