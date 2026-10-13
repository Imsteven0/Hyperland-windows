//! WebView2 apps (the new WhatsApp, Teams…) draw their content in a separate,
//! unowned, layered popup of `msedgewebview2.exe`. When komorebi (or a virtual
//! desktop switch) cloaks the app's window, that popup stays on screen:
//! invisible, but swallowing every click over it, so the desktop and the
//! windows below stop responding. While its app is hidden we make the popup
//! click-through, and undo that as soon as the app shows again or the shell exits.

use std::collections::{HashMap, HashSet};

use windows::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM},
    Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
    System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS},
    UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
        SetWindowLongPtrW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
    },
};
use windows::core::BOOL;

struct Top {
    hwnd: HWND,
    pid: u32,
    ex: u32,
    owned: bool,
    /// Cloaked (komorebi / other virtual desktop) or minimized.
    hidden: bool,
}

impl Top {
    /// A real application window (what komorebi manages and cloaks).
    fn is_app(&self) -> bool {
        !self.owned && self.ex & WS_EX_TOOLWINDOW.0 == 0
    }

    /// The WebView2 content popup: layered, never activated, unowned, on screen.
    fn is_webview_overlay(&self) -> bool {
        let want = WS_EX_LAYERED.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0;
        !self.owned && !self.hidden && self.ex & want == want && class_name(self.hwnd) == "Chrome_WidgetWin_1"
    }
}

#[derive(Default)]
pub struct GhostGuard {
    /// Overlays we made click-through (HWND values).
    patched: HashSet<isize>,
}

impl GhostGuard {
    pub fn scan(&mut self) {
        let tops = top_windows();
        let overlays: Vec<&Top> = tops.iter().filter(|t| t.is_webview_overlay()).collect();
        let mut want = HashSet::new();
        if !overlays.is_empty() {
            let procs = processes();
            for o in overlays {
                if procs.get(&o.pid).is_none_or(|(_, exe)| exe != "msedgewebview2.exe") {
                    continue;
                }
                // The host app launched the WebView2 browser process (renderers sit lower).
                let mut pid = o.pid;
                let host = (0..4).find_map(|_| {
                    pid = procs.get(&pid)?.0;
                    tops.iter().any(|t| t.pid == pid && t.is_app()).then_some(pid)
                });
                let Some(host) = host else { continue };
                if tops.iter().filter(|t| t.pid == host && t.is_app()).all(|t| t.hidden) {
                    want.insert(o.hwnd.0 as isize);
                }
            }
        }
        for &h in &want {
            // Re-applied if the app reset its styles meanwhile.
            if set_click_through(HWND(h as _), true) || self.patched.contains(&h) {
                self.patched.insert(h);
            }
        }
        let stale: Vec<isize> = self.patched.difference(&want).copied().collect();
        for h in stale {
            set_click_through(HWND(h as _), false);
            self.patched.remove(&h);
        }
    }

    pub fn restore_all(&mut self) {
        for h in self.patched.drain() {
            set_click_through(HWND(h as _), false);
        }
    }
}

/// Adds/removes WS_EX_TRANSPARENT; true when the style changed.
fn set_click_through(hwnd: HWND, on: bool) -> bool {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let t = WS_EX_TRANSPARENT.0 as isize;
        let new = if on { ex | t } else { ex & !t };
        if new == ex {
            return false;
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new);
        log::info!("webview overlay {:?} click-through: {on}", hwnd.0);
        true
    }
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn top_windows() -> Vec<Top> {
    unsafe extern "system" fn each(hwnd: HWND, l: LPARAM) -> BOOL {
        let out = unsafe { &mut *(l.0 as *mut Vec<Top>) };
        unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                let mut cloaked = 0u32;
                let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4);
                out.push(Top {
                    hwnd,
                    pid,
                    ex: GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32,
                    owned: GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid()),
                    hidden: cloaked != 0 || IsIconic(hwnd).as_bool(),
                });
            }
        }
        BOOL(1)
    }
    let mut out: Vec<Top> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// pid -> (parent pid, lowercase exe name).
fn processes() -> HashMap<u32, (u32, String)> {
    let mut map = HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return map };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            map.insert(e.th32ProcessID, (e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase()));
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    map
}
