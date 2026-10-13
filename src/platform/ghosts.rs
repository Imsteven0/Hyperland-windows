//! WebView2 apps (the new WhatsApp, Teams…) draw their content in a separate,
//! unowned, layered popup of `msedgewebview2.exe`. When komorebi (or a virtual
//! desktop switch) cloaks the app's window, that popup stays on screen:
//! invisible, but swallowing every click over it, so the desktop and the
//! windows below stop responding. While its app is hidden we make the popup
//! click-through, and undo that as soon as the app shows again or the shell exits.
//!
//! The patched windows are also written to disk, so that if the shell
//! crashes or is killed while one is click-through, the next start (or the
//! panic hook) puts it back: the app never stays "see-through" for good.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

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

/// A window we made click-through, as remembered on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patched {
    pub hwnd: isize,
    pub pid: u32,
}

#[derive(Default)]
pub struct GhostGuard {
    /// Overlays we made click-through: HWND value -> owning pid.
    patched: HashMap<isize, u32>,
    /// Where the list is mirrored (None: memory only).
    file: Option<PathBuf>,
}

impl GhostGuard {
    /// A guard that mirrors its list to `file`, after undoing whatever a
    /// previous run left behind there.
    pub fn with_state(file: PathBuf) -> Self {
        let restored = restore_from_disk(&file);
        if restored > 0 {
            log::warn!("restored {restored} window(s) left click-through by a previous run");
        }
        Self { patched: HashMap::new(), file: Some(file) }
    }

    fn persist(&self) {
        let Some(file) = &self.file else { return };
        let list: Vec<Patched> = self.patched.iter().map(|(&hwnd, &pid)| Patched { hwnd, pid }).collect();
        let result = if list.is_empty() {
            match std::fs::remove_file(file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            }
        } else {
            serde_json::to_string(&list).map_err(anyhow::Error::from).and_then(|t| crate::config::write_atomic(file, &t))
        };
        if let Err(e) = result {
            log::warn!("saving click-through windows: {e:#}");
        }
    }

    pub fn scan(&mut self) {
        let tops = top_windows();
        let overlays: Vec<&Top> = tops.iter().filter(|t| t.is_webview_overlay()).collect();
        let mut want: HashMap<isize, u32> = HashMap::new();
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
                    want.insert(o.hwnd.0 as isize, o.pid);
                }
            }
        }
        let before: HashSet<isize> = self.patched.keys().copied().collect();
        for (&h, &pid) in &want {
            // Re-applied if the app reset its styles meanwhile.
            if set_click_through(HWND(h as _), true) || self.patched.contains_key(&h) {
                self.patched.insert(h, pid);
            }
        }
        let stale: Vec<isize> = self.patched.keys().filter(|h| !want.contains_key(h)).copied().collect();
        for h in stale {
            set_click_through(HWND(h as _), false);
            self.patched.remove(&h);
        }
        if self.patched.keys().copied().collect::<HashSet<_>>() != before {
            self.persist();
        }
    }

    pub fn restore_all(&mut self) {
        for (h, _) in self.patched.drain() {
            set_click_through(HWND(h as _), false);
        }
        self.persist();
    }
}

/// Undoes click-through for the windows listed in `file` (left by a run that
/// crashed or was killed) and deletes it. Safe to call from a panic hook.
/// Returns how many windows were restored.
pub fn restore_from_disk(file: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(file) else { return 0 };
    let list: Vec<Patched> = serde_json::from_str(&text).unwrap_or_default();
    let restored = list
        .iter()
        .filter(|p| {
            let hwnd = HWND(p.hwnd as _);
            // HWNDs get reused: only touch the very same overlay window.
            still_ours(p, window_pid(hwnd), &class_name(hwnd)) && set_click_through(hwnd, false)
        })
        .count();
    let _ = std::fs::remove_file(file);
    restored
}

/// Whether a remembered window is still the WebView2 overlay we patched.
fn still_ours(p: &Patched, live_pid: Option<u32>, class: &str) -> bool {
    live_pid == Some(p.pid) && class == "Chrome_WidgetWin_1"
}

fn window_pid(hwnd: HWND) -> Option<u32> {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (pid != 0).then_some(pid)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_same_overlay_is_restored() {
        let p = Patched { hwnd: 0x1234, pid: 42 };
        assert!(still_ours(&p, Some(42), "Chrome_WidgetWin_1"));
        assert!(!still_ours(&p, Some(43), "Chrome_WidgetWin_1"), "HWND reused by another process");
        assert!(!still_ours(&p, Some(42), "Notepad"), "HWND reused by another window class");
        assert!(!still_ours(&p, None, "Chrome_WidgetWin_1"), "window is gone");
    }

    #[test]
    fn list_roundtrips_and_restore_clears_the_file() {
        let file = std::env::temp_dir().join(format!("caelestia-ghosts-{}.json", std::process::id()));
        let mut guard = GhostGuard { patched: HashMap::from([(0x7fff_0001, 4242)]), file: Some(file.clone()) };
        guard.persist();
        let saved: Vec<Patched> = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(saved, [Patched { hwnd: 0x7fff_0001, pid: 4242 }]);
        // That window doesn't exist here: nothing is touched, the file goes away.
        assert_eq!(restore_from_disk(&file), 0);
        assert!(!file.exists());
        assert_eq!(restore_from_disk(&file), 0, "no file, nothing to do");
        // An empty list removes the file instead of writing "[]".
        guard.patched.clear();
        guard.persist();
        assert!(!file.exists());
    }

    /// End to end: a crashed run left a real overlay click-through.
    /// Needs a desktop session: `cargo test -- --ignored click_through`.
    #[test]
    #[ignore]
    fn restores_a_window_left_click_through() {
        use windows::{
            Win32::{
                System::LibraryLoader::GetModuleHandleW,
                UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WINDOW_EX_STYLE, WNDCLASSW, WS_POPUP},
            },
            core::w,
        };
        unsafe extern "system" fn proc(h: HWND, m: u32, w: windows::Win32::Foundation::WPARAM, l: LPARAM) -> windows::Win32::Foundation::LRESULT {
            unsafe { DefWindowProcW(h, m, w, l) }
        }
        unsafe {
            let instance = GetModuleHandleW(None).unwrap().into();
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(proc), hInstance: instance, lpszClassName: w!("Chrome_WidgetWin_1"), ..Default::default() });
            let ex = WINDOW_EX_STYLE(WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0);
            let hwnd = CreateWindowExW(ex, w!("Chrome_WidgetWin_1"), w!("overlay"), WS_POPUP, 0, 0, 10, 10, None, None, Some(instance), None).unwrap();
            let file = std::env::temp_dir().join(format!("caelestia-ghosts-e2e-{}.json", std::process::id()));
            std::fs::write(&file, serde_json::to_string(&[Patched { hwnd: hwnd.0 as isize, pid: std::process::id() }]).unwrap()).unwrap();
            assert_eq!(restore_from_disk(&file), 1);
            assert_eq!(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0, 0, "click-through removed");
            assert!(!file.exists());
            let _ = DestroyWindow(hwnd);
        }
    }
}
