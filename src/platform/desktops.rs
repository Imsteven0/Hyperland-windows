//! Workspaces = Windows virtual desktops, plus the active-window tracker.
//! Desktop ids come from the (stable, documented-by-usage) Explorer registry
//! keys; occupancy uses the public `IVirtualDesktopManager`; switching sends
//! the standard Ctrl+Win+Arrow shortcut, so no undocumented COM is needed.

use std::cell::RefCell;

use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM},
        System::{
            Com::{CLSCTX_ALL, CoCreateInstance},
            Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
        },
        UI::{
            Accessibility::{HWINEVENTHOOK, SetWinEventHook},
            Shell::{IVirtualDesktopManager, VirtualDesktopManager},
            WindowsAndMessaging::{
                EVENT_OBJECT_NAMECHANGE, EVENT_SYSTEM_FOREGROUND, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongPtrW,
                GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
                WINEVENT_SKIPOWNPROCESS, WS_EX_TOOLWINDOW,
            },
        },
    },
    core::{BOOL, GUID, PWSTR},
};

use super::system::{HKCU, reg_binary};

const VD_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\VirtualDesktops";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Desktops {
    /// One entry per desktop: whether any app window lives on it.
    pub occupied: Vec<bool>,
    /// App windows (HWND values) per desktop, in z-order.
    pub windows: Vec<Vec<isize>>,
    /// 1-based index of the current desktop.
    pub current: usize,
}

fn guid_from(bytes: &[u8]) -> GUID {
    let d1 = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let d2 = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    let d3 = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
    GUID::from_values(d1, d2, d3, bytes[8..16].try_into().unwrap())
}

fn desktop_ids() -> Vec<GUID> {
    reg_binary(HKCU, VD_KEY, "VirtualDesktopIDs").map(|b| b.chunks_exact(16).map(guid_from).collect()).unwrap_or_default()
}

fn current_desktop_id() -> Option<GUID> {
    if let Some(b) = reg_binary(HKCU, VD_KEY, "CurrentVirtualDesktop").filter(|b| b.len() == 16) {
        return Some(guid_from(&b));
    }
    // Windows 10 stores it per session.
    let session = unsafe {
        let mut sid = 0u32;
        let _ = windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(std::process::id(), &mut sid);
        sid
    };
    let key = format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\SessionInfo\{session}\VirtualDesktops");
    reg_binary(HKCU, &key, "CurrentVirtualDesktop").filter(|b| b.len() == 16).map(|b| guid_from(&b))
}

thread_local! {
    static VDM: RefCell<Option<IVirtualDesktopManager>> = const { RefCell::new(None) };
}

/// Must be called on a COM-initialised thread (the UI thread is, via winit/OLE).
pub fn query() -> Desktops {
    let ids = desktop_ids();
    if ids.is_empty() {
        return Desktops { occupied: vec![true], windows: vec![app_windows().into_iter().map(|h| h.0 as isize).collect()], current: 1 };
    }
    let current = current_desktop_id().and_then(|c| ids.iter().position(|g| *g == c)).map(|i| i + 1).unwrap_or(1);

    let mut occupied = vec![false; ids.len()];
    let mut windows: Vec<Vec<isize>> = vec![Vec::new(); ids.len()];
    // Clone the interface out so no RefCell borrow is held across COM calls:
    // cross-apartment calls pump messages and can re-enter this function.
    let vdm = VDM.with(|vdm| {
        let mut vdm = vdm.borrow_mut();
        if vdm.is_none() {
            *vdm = unsafe { CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_ALL).ok() };
        }
        vdm.clone()
    });
    if let Some(vdm) = vdm {
        for hwnd in app_windows() {
            if let Ok(id) = unsafe { vdm.GetWindowDesktopId(hwnd) } {
                if let Some(i) = ids.iter().position(|g| *g == id) {
                    occupied[i] = true;
                    windows[i].push(hwnd.0 as isize);
                }
            }
        }
    }
    Desktops { occupied, windows, current }
}

/// Switches to the 1-based desktop `target` using Ctrl+Win+Left/Right.
pub fn switch_to(target: usize, current: usize, count: usize) {
    if target == 0 || target > count || target == current {
        return;
    }
    const VK_CONTROL: u16 = 0x11;
    const VK_LWIN: u16 = 0x5B;
    let (arrow, n) = if target > current { (0x27u16, target - current) } else { (0x25u16, current - target) };
    super::window::send_keys(&[VK_CONTROL, VK_LWIN], &vec![arrow; n]);
}

fn app_windows() -> Vec<HWND> {
    unsafe extern "system" fn cb(hwnd: HWND, lp: LPARAM) -> BOOL {
        unsafe {
            let out = &mut *(lp.0 as *mut Vec<HWND>);
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if IsWindowVisible(hwnd).as_bool()
                && GetWindow(hwnd, GW_OWNER).map(|o| o.is_invalid()).unwrap_or(true)
                && ex & WS_EX_TOOLWINDOW.0 == 0
                && GetWindowTextLengthW(hwnd) > 0
            {
                out.push(hwnd);
            }
        }
        true.into()
    }
    let mut out: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

// ---------------- active window ----------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActiveWindow {
    pub title: String,
    pub app: String,
}

pub fn window_info(hwnd: HWND) -> Option<ActiveWindow> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == std::process::id() || pid == 0 {
            return None; // our own surfaces never count as "active window"
        }
        let len = GetWindowTextLengthW(hwnd);
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        let app = process_name(pid).unwrap_or_default();
        if title.is_empty() && app.is_empty() {
            return None;
        }
        Some(ActiveWindow { title: if title.is_empty() { app.clone() } else { title }, app })
    }
}

fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| {
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(path)
        })
    }
}

thread_local! {
    static ON_FOREGROUND: RefCell<Option<std::rc::Rc<dyn Fn(HWND)>>> = const { RefCell::new(None) };
}

unsafe extern "system" fn win_event(_: HWINEVENTHOOK, event: u32, hwnd: HWND, id_object: i32, _: i32, _: u32, _: u32) {
    if event == EVENT_OBJECT_NAMECHANGE && (id_object != OBJID_WINDOW.0 || hwnd != super::window::foreground_window()) {
        return;
    }
    // Never run app code inside the hook: it can fire while a COM call is
    // pumping messages. Defer to a clean stack on the event loop instead.
    let Some(f) = ON_FOREGROUND.with(|f| f.borrow().clone()) else { return };
    let raw = hwnd.0 as isize;
    slint::Timer::single_shot(std::time::Duration::ZERO, move || f(HWND(raw as *mut _)));
}

/// Calls `f` on the UI thread whenever the foreground window (or its title) changes.
/// Out-of-context hooks are delivered through the calling thread's message loop.
pub fn watch_foreground(f: impl Fn(HWND) + 'static) {
    ON_FOREGROUND.with(|slot| *slot.borrow_mut() = Some(std::rc::Rc::new(f)));
    unsafe {
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(win_event), 0, 0, flags);
        SetWinEventHook(EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE, None, Some(win_event), 0, 0, flags);
    }
}
