//! Win32 window plumbing: turning Slint/winit windows into shell surfaces
//! (no taskbar button, no activation, click-through), AppBar space
//! reservation, monitor geometry and DWM backdrops.

use std::{ffi::c_void, mem::size_of, sync::Once};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            Dwm::{
                DWM_SYSTEMBACKDROP_TYPE, DWM_WINDOW_CORNER_PREFERENCE, DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_EXCLUDED_FROM_PEEK,
                DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
                DWMWCP_DONOTROUND, DWMWCP_ROUND, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
            },
            Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::MARGINS,
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetProcessDpiAwarenessContext},
            Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY},
            Shell::{ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, APPBARDATA, SHAppBarMessage},
            WindowsAndMessaging::{
                AllowSetForegroundWindow, BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, GWL_EXSTYLE, GetCursorPos,
                GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId, HWND_TOPMOST, RegisterClassW,                 SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos,                 WNDCLASSW, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
            },
        },
    },
    core::{BOOL, PCWSTR, w},
};

pub fn init_dpi_awareness() {
    // winit does this too, but we query monitors before the event loop exists.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn hwnd_of(window: &slint::Window) -> Option<HWND> {
    let handle = window.window_handle();
    let raw = handle.window_handle().ok()?.as_raw();
    match raw {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceStyle {
    /// Mouse events fall through to whatever is below (the screen frame).
    pub click_through: bool,
    /// Never steal focus from the user's apps (bar, dashboard, utilities).
    pub no_activate: bool,
    pub topmost: bool,
}

// Bits of `dwRefData` for the subclass proc.
const F_CLICK_THROUGH: usize = 1;
const F_NO_ACTIVATE: usize = 2;
const SUBCLASS_ID: usize = 0xCAE1;

fn wanted_ex(flags: usize) -> (u32, u32) {
    let mut add = WS_EX_TOOLWINDOW.0;
    let mut remove = WS_EX_APPWINDOW.0;
    if flags & F_NO_ACTIVATE != 0 {
        add |= WS_EX_NOACTIVATE.0;
    } else {
        remove |= WS_EX_NOACTIVATE.0;
    }
    if flags & F_CLICK_THROUGH != 0 {
        add |= WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0;
    } else {
        remove |= WS_EX_TRANSPARENT.0;
    }
    (add, remove)
}

/// winit rewrites GWL_EXSTYLE wholesale whenever it updates a window, so a
/// one-off SetWindowLongPtr doesn't stick. This subclass proc patches every
/// style change and refuses mouse activation for non-focusable surfaces.
unsafe extern "system" fn surface_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, flags: usize) -> LRESULT {
    use windows::Win32::UI::{Shell::DefSubclassProc, WindowsAndMessaging::{MA_NOACTIVATE, STYLESTRUCT, WM_MOUSEACTIVATE, WM_STYLECHANGING}};
    unsafe {
        match msg {
            WM_STYLECHANGING if wp.0 as i32 == GWL_EXSTYLE.0 => {
                let ss = &mut *(lp.0 as *mut STYLESTRUCT);
                let (add, remove) = wanted_ex(flags);
                ss.styleNew = (ss.styleNew | add) & !remove;
            }
            WM_MOUSEACTIVATE if flags & F_NO_ACTIVATE != 0 => return LRESULT(MA_NOACTIVATE as isize),
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}

/// Turns a window into a shell surface: hidden from taskbar/Alt-Tab/Peek,
/// without DWM open/close animations, optionally click-through.
pub fn make_surface(hwnd: HWND, style: SurfaceStyle) {
    use windows::Win32::UI::Shell::SetWindowSubclass;
    let flags = if style.click_through { F_CLICK_THROUGH } else { 0 } | if style.no_activate { F_NO_ACTIVATE } else { 0 };
    unsafe {
        // Re-installing with the same id just updates the ref data.
        let _ = SetWindowSubclass(hwnd, Some(surface_proc), SUBCLASS_ID, flags);
        let (add, remove) = wanted_ex(flags);
        let ex = (GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 | add) & !remove;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
        if style.click_through {
            use windows::Win32::UI::WindowsAndMessaging::{LWA_ALPHA, SetLayeredWindowAttributes};
            let _ = SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 255, LWA_ALPHA);
        }
        let on: BOOL = true.into();
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, &on as *const _ as *const c_void, size_of::<BOOL>() as u32);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_EXCLUDED_FROM_PEEK, &on as *const _ as *const c_void, size_of::<BOOL>() as u32);
        let corner: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const c_void, size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32);
        no_dwm_border(hwnd);
        if style.topmost {
            set_topmost(hwnd);
        }
    }
}

/// Click-through for the screen frame without WS_EX_LAYERED (which breaks
/// OpenGL composition): the window region is the frame only, so clicks in the
/// hole reach the apps below. The hole is inset by 2px so the antialiased
/// inner edge drawn by Slint stays inside the region.
pub fn set_frame_region(hwnd: HWND, width: i32, height: i32, left: i32, thickness: i32, radius: i32) {
    use windows::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, HGDIOBJ, RGN_DIFF, SetWindowRgn};
    const INSET: i32 = 2;
    unsafe {
        let outer = CreateRectRgn(left, 0, width, height); // the bar paints its own strip
        let r = (radius - INSET).max(0) * 2 + 1;
        let hole = CreateRoundRectRgn(left + INSET, thickness + INSET, width - thickness - INSET + 1, height - thickness - INSET + 1, r, r);
        CombineRgn(Some(outer), Some(outer), Some(hole), RGN_DIFF);
        let _ = DeleteObject(HGDIOBJ(hole.0));
        // The system owns `outer` after a successful SetWindowRgn.
        if SetWindowRgn(hwnd, Some(outer), true) == 0 {
            let _ = DeleteObject(HGDIOBJ(outer.0));
        }
    }
}

/// Gaussian blur of whatever is behind the window, clipped to its region
/// (`SetWindowCompositionAttribute` + `ACCENT_ENABLE_BLURBEHIND`). Unlike the
/// Win11 system backdrops this works on borderless, region-shaped windows.
pub fn set_blur_behind(hwnd: HWND, on: bool) {
    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        gradient: u32,
        animation: u32,
    }
    #[repr(C)]
    struct CompositionData {
        attrib: u32,
        data: *mut c_void,
        size: usize,
    }
    type SetWca = unsafe extern "system" fn(HWND, *mut CompositionData) -> BOOL;
    const WCA_ACCENT_POLICY: u32 = 19;
    const ACCENT_DISABLED: u32 = 0;
    const ACCENT_ENABLE_BLURBEHIND: u32 = 3;
    unsafe {
        use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        let Ok(user32) = LoadLibraryW(w!("user32.dll")) else { return };
        let Some(f) = GetProcAddress(user32, windows::core::s!("SetWindowCompositionAttribute")) else { return };
        let set: SetWca = std::mem::transmute(f);
        let mut policy = AccentPolicy { state: if on { ACCENT_ENABLE_BLURBEHIND } else { ACCENT_DISABLED }, flags: 0, gradient: 0, animation: 0 };
        let mut data = CompositionData { attrib: WCA_ACCENT_POLICY, data: &mut policy as *mut _ as *mut c_void, size: size_of::<AccentPolicy>() };
        let _ = set(hwnd, &mut data);
    }
}

pub fn set_topmost(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

/// Windows 11 draws a 1px accent/grey border around every top-level window;
/// shell surfaces draw their own edges.
pub fn no_dwm_border(hwnd: HWND) {
    const DWMWA_BORDER_COLOR: windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE = windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(34);
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &DWMWA_COLOR_NONE as *const _ as *const c_void, 4);
    }
}

/// DWM system backdrop (acrylic) for a window. Shell surfaces don't use it any
/// more: DWM clips it to its own fixed 8px corners and adds a grey border that
/// clash with Caelestia's large radii; blur comes from `BlurPane`s instead.
pub fn set_backdrop(hwnd: HWND, acrylic: bool, dark: bool) {
    unsafe {
        let margins = if acrylic { MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 } } else { MARGINS::default() };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        let kind: DWM_SYSTEMBACKDROP_TYPE = if acrylic { DWMSBT_TRANSIENTWINDOW } else { DWMSBT_NONE };
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &kind as *const _ as *const c_void, size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32);
        let dark: BOOL = dark.into();
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const c_void, size_of::<BOOL>() as u32);
        let corner = if acrylic { DWMWCP_ROUND } else { DWMWCP_DONOTROUND };
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const c_void, size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32);
        no_dwm_border(hwnd);
    }
}

/// Brings one of our windows to the foreground so it receives keyboard input.
///
/// Windows only lets the process that received the last input event change
/// the foreground window. With a low-level keyboard hook the key went to the
/// app the user was in, so a plain `SetForegroundWindow` is silently ignored.
/// Attaching our input queue to the foreground thread's makes the switch
/// legitimate; an injected no-op key is the fallback. Returns success.
pub fn focus_window(hwnd: HWND) -> bool {
    use windows::Win32::{
        System::Threading::{AttachThreadInput, GetCurrentThreadId},
        UI::{Input::KeyboardAndMouse::{SetActiveWindow, SetFocus}, WindowsAndMessaging::{SW_SHOW, ShowWindow}},
    };
    unsafe {
        if GetForegroundWindow() == hwnd {
            return true;
        }
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        let attached = fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetActiveWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        if attached {
            let _ = AttachThreadInput(me, fg_thread, false);
        }
        if GetForegroundWindow() != hwnd {
            // Fallback: an injected key (an unassigned VK, harmless even while
            // the user still holds Alt) grants our process foreground rights.
            let _ = AllowSetForegroundWindow(std::process::id());
            send_keys(&[], &[0xE8]);
            let _ = SetForegroundWindow(hwnd);
        }
        GetForegroundWindow() == hwnd
    }
}

/// Presses `down` keys in order then releases them in reverse (+ `taps` in between).
pub fn send_keys(chord: &[u16], taps: &[u16]) {
    let key = |vk: u16, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 },
        },
    };
    let mut inputs = Vec::new();
    inputs.extend(chord.iter().map(|&vk| key(vk, false)));
    for &t in taps {
        inputs.push(key(t, false));
        inputs.push(key(t, true));
    }
    inputs.extend(chord.iter().rev().map(|&vk| key(vk, true)));
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

pub fn foreground_window() -> HWND {
    unsafe { GetForegroundWindow() }
}

/// Whether the foreground app is full screen (game, video, presentation).
/// The shell's notification state covers exclusive/D3D full screen; borderless
/// windows are caught by comparing the window with its monitor.
pub fn fullscreen_foreground() -> bool {
    use windows::Win32::{
        Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromWindow},
        UI::{
            Shell::{QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState},
            WindowsAndMessaging::{GWL_STYLE, GetClassNameW, GetWindowRect, IsWindowVisible, IsZoomed, WS_CAPTION},
        },
    };
    let hwnd = foreground_window();
    if hwnd.is_invalid() {
        return false;
    }
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == std::process::id() {
            return false; // our own drawers
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut class) as usize;
        // The desktop itself, and shell overlays that cover the monitor without a
        // caption: Task View / Alt+Tab (Win+Tab, or Cmd+Tab passed on by a VM),
        // Start, search.
        if matches!(
            String::from_utf16_lossy(&class[..n]).as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "XamlExplorerHostIslandWindow" | "MultitaskingViewFrame" | "TaskSwitcherWnd" | "Windows.UI.Core.CoreWindow"
        ) {
            return false;
        }
        if matches!(SHQueryUserNotificationState(), Ok(QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE)) {
            return true;
        }
        // Maximised windows keep their caption, except UWP frames
        // (ApplicationFrameHost) that draw their own: check both.
        if !IsWindowVisible(hwnd).as_bool() || IsZoomed(hwnd).as_bool() || GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_CAPTION.0 == WS_CAPTION.0 {
            return false;
        }
        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL);
        if hmon.is_invalid() {
            return false;
        }
        let mon = monitor_info(hmon).rect;
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r).is_ok() && covers(Rect::from(r), mon)
    }
}

/// A window "covers" a monitor when it spans all of it (overshoot allowed).
fn covers(win: Rect, mon: Rect) -> bool {
    win.left <= mon.left && win.top <= mon.top && win.right >= mon.right && win.bottom >= mon.bottom
}

#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    /// GDI device name, e.g. `\.\DISPLAY2` (used by `bar.excludedScreens`).
    pub name: String,
    /// Full monitor rectangle in physical pixels.
    pub rect: Rect,
    pub scale: f32,
    pub primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
    pub fn inflate(&self, by: i32) -> Rect {
        Rect { left: self.left - by, top: self.top - by, right: self.right + by, bottom: self.bottom + by }
    }
}

impl From<RECT> for Rect {
    fn from(r: RECT) -> Self {
        Self { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
    }
}

fn monitor_info(hmon: windows::Win32::Graphics::Gdi::HMONITOR) -> Monitor {
    use windows::Win32::Graphics::Gdi::MONITORINFOEXW;
    unsafe {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        let _ = GetMonitorInfoW(hmon, &mut info as *mut _ as *mut MONITORINFO);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let name = String::from_utf16_lossy(&info.szDevice[..info.szDevice.iter().position(|&c| c == 0).unwrap_or(0)]);
        Monitor { name, rect: info.monitorInfo.rcMonitor.into(), scale: dx as f32 / 96.0, primary: info.monitorInfo.dwFlags & 1 != 0 }
    }
}

pub fn primary_monitor() -> Monitor {
    unsafe { monitor_info(MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)) }
}

/// All monitors, primary first, then left-to-right.
pub fn monitors() -> Vec<Monitor> {
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
    unsafe extern "system" fn cb(hmon: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        unsafe { (*(lp.0 as *mut Vec<Monitor>)).push(monitor_info(hmon)) };
        true.into()
    }
    let mut out: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out.sort_by_key(|m| (!m.primary, m.rect.left, m.rect.top));
    if out.is_empty() {
        out.push(primary_monitor());
    }
    out
}

/// The part of a monitor the shell may draw on: the monitor minus any
/// Windows taskbar (primary or secondary) docked on it, unless auto-hidden.
pub fn shell_area(mon: &Monitor) -> Rect {
    use windows::Win32::UI::{Shell::{ABM_GETSTATE, ABS_AUTOHIDE}, WindowsAndMessaging::{FindWindowExW, FindWindowW, GetWindowRect}};
    let mut area = mon.rect;
    unsafe {
        let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, ..Default::default() };
        if SHAppBarMessage(ABM_GETSTATE, &mut abd) as u32 & ABS_AUTOHIDE != 0 {
            return area;
        }
        let mut bars = Vec::new();
        if let Ok(h) = FindWindowW(w!("Shell_TrayWnd"), None) {
            bars.push(h);
        }
        let mut prev: Option<HWND> = None;
        while let Ok(h) = FindWindowExW(None, prev, w!("Shell_SecondaryTrayWnd"), None) {
            bars.push(h);
            prev = Some(h);
        }
        for h in bars {
            let mut rc = RECT::default();
            if GetWindowRect(h, &mut rc).is_err() {
                continue;
            }
            let tb: Rect = rc.into();
            let (cx, cy) = ((tb.left + tb.right) / 2, (tb.top + tb.bottom) / 2);
            if !mon.rect.contains(cx, cy) {
                continue;
            }
            if tb.width() >= tb.height() {
                if cy < (mon.rect.top + mon.rect.bottom) / 2 { area.top = area.top.max(tb.bottom) } else { area.bottom = area.bottom.min(tb.top) }
            } else if cx < (mon.rect.left + mon.rect.right) / 2 {
                area.left = area.left.max(tb.right)
            } else {
                area.right = area.right.min(tb.left)
            }
        }
    }
    area
}

// ---------------------------------------------------------------------------
// AppBar: reserves screen edges so maximised windows don't cover the shell.
// Each edge needs its own HWND; we use tiny hidden helper windows so the
// reservation is independent of the Slint windows' lifetime and visibility.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Top,
    Right,
    Bottom,
}

pub struct AppBar {
    hwnd: HWND,
    edge: Edge,
    registered: bool,
}

unsafe extern "system" fn helper_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

fn helper_class() -> PCWSTR {
    static REGISTER: Once = Once::new();
    let name = w!("CaelestiaAppBarHelper");
    REGISTER.call_once(|| unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(helper_proc),
            hInstance: GetModuleHandleW(None).map(Into::into).unwrap_or_default(),
            lpszClassName: name,
            ..Default::default()
        };
        RegisterClassW(&wc);
    });
    name
}

impl AppBar {
    pub fn new(edge: Edge) -> Option<Self> {
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                helper_class(),
                w!("caelestia-appbar"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                GetModuleHandleW(None).ok().map(Into::into),
                None,
            )
            .ok()?;
            let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: hwnd, ..Default::default() };
            let registered = SHAppBarMessage(ABM_NEW, &mut abd) != 0;
            Some(Self { hwnd, edge, registered })
        }
    }

    pub fn edge(&self) -> Edge {
        self.edge
    }

    /// Reserves `thickness` physical pixels along this edge of `mon`.
    pub fn reserve(&self, mon: &Rect, thickness: i32) {
        if !self.registered {
            return;
        }
        let (edge, rc) = match self.edge {
            Edge::Left => (ABE_LEFT, RECT { left: mon.left, top: mon.top, right: mon.left + thickness, bottom: mon.bottom }),
            Edge::Top => (ABE_TOP, RECT { left: mon.left, top: mon.top, right: mon.right, bottom: mon.top + thickness }),
            Edge::Right => (ABE_RIGHT, RECT { left: mon.right - thickness, top: mon.top, right: mon.right, bottom: mon.bottom }),
            Edge::Bottom => (ABE_BOTTOM, RECT { left: mon.left, top: mon.bottom - thickness, right: mon.right, bottom: mon.bottom }),
        };
        unsafe {
            let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: self.hwnd, uEdge: edge, rc, ..Default::default() };
            SHAppBarMessage(ABM_QUERYPOS, &mut abd);
            // QUERYPOS may shrink the rect (e.g. around the taskbar); keep the requested thickness.
            match self.edge {
                Edge::Left => abd.rc.right = abd.rc.left + thickness,
                Edge::Top => abd.rc.bottom = abd.rc.top + thickness,
                Edge::Right => abd.rc.left = abd.rc.right - thickness,
                Edge::Bottom => abd.rc.top = abd.rc.bottom - thickness,
            }
            SHAppBarMessage(ABM_SETPOS, &mut abd);
        }
    }
}

impl Drop for AppBar {
    fn drop(&mut self) {
        unsafe {
            if self.registered {
                let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: self.hwnd, ..Default::default() };
                SHAppBarMessage(ABM_REMOVE, &mut abd);
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_screen_coverage() {
        let mon = Rect { left: 0, top: 0, right: 1920, bottom: 1080 };
        assert!(covers(mon, mon));
        assert!(covers(Rect { left: -8, top: -8, right: 1928, bottom: 1088 }, mon));
        assert!(!covers(Rect { left: 0, top: 0, right: 1920, bottom: 1040 }, mon), "work area only (taskbar visible)");
        let second = Rect { left: 1920, top: 0, right: 3840, bottom: 1080 };
        assert!(!covers(mon, second));
    }

    #[test]
    fn rect_helpers() {
        let r = Rect { left: 10, top: 20, right: 110, bottom: 70 };
        assert_eq!((r.width(), r.height()), (100, 50));
        assert!(r.contains(10, 20) && !r.contains(110, 70));
        assert_eq!(r.inflate(5), Rect { left: 5, top: 15, right: 115, bottom: 75 });
    }
}
