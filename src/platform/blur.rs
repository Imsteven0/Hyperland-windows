//! Blur behind translucent panels.
//!
//! DWM's blur (accent policy / system backdrops) always covers the whole
//! window rectangle and ignores window regions, while our drawers are
//! irregular (flares, animated extent, 25px corners). So each panel gets a
//! `BlurPane`: a plain, rectangular, DWM-rounded window carrying the blur,
//! kept directly *below* the panel window and resized to follow the panel as
//! it animates. The pane is inset on the panel's free edges so its 8px corners
//! always stay inside the panel's larger rounded corners.

use std::{cell::Cell, ffi::c_void, mem::size_of, sync::Once};

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        Graphics::{
            Dwm::{DWM_WINDOW_CORNER_PREFERENCE, DWMWA_EXCLUDED_FROM_PEEK, DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute},
            Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow, WNDCLASSW, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        },
    },
    core::{BOOL, PCWSTR, w},
};

use super::window::{Rect, no_dwm_border, set_blur_behind};

/// DWM's corner radius for `DWMWCP_ROUND`, in 96-dpi pixels.
const PANE_RADIUS: f32 = 8.0;

/// Which panel edges are glued to the screen frame (no rounded corners there).
#[derive(Debug, Clone, Copy, Default)]
pub struct Attached {
    pub left: bool,
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
}

unsafe extern "system" fn pane_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

fn pane_class() -> PCWSTR {
    static REGISTER: Once = Once::new();
    let name = w!("CaelestiaBlurPane");
    REGISTER.call_once(|| unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(pane_proc),
            hInstance: GetModuleHandleW(None).map(Into::into).unwrap_or_default(),
            lpszClassName: name,
            // Black paints as fully transparent under the blur accent.
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            ..Default::default()
        };
        RegisterClassW(&wc);
    });
    name
}

pub struct BlurPane {
    hwnd: HWND,
    last: Cell<Option<Rect>>,
}

impl BlurPane {
    pub fn new() -> Option<Self> {
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                pane_class(),
                w!("caelestia-blur"),
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
            set_blur_behind(hwnd, true);
            let corner: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const c_void, size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32);
            let on: BOOL = true.into();
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, &on as *const _ as *const c_void, size_of::<BOOL>() as u32);
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_EXCLUDED_FROM_PEEK, &on as *const _ as *const c_void, size_of::<BOOL>() as u32);
            no_dwm_border(hwnd);
            Some(Self { hwnd, last: Cell::new(None) })
        }
    }

    /// Shows the pane under `panel` covering `rect` (screen px) shrunk on the
    /// free edges by enough to hide its corners inside a `radius` px rounding.
    pub fn follow(&self, panel: HWND, rect: Rect, attached: Attached, radius: f32, scale: f32) {
        // Along the 45° diagonal the panel's arc sits R(√2−1) from the corner,
        // the pane's at d√2 + r(√2−1): solve for the inset d.
        let d = (((radius - PANE_RADIUS * scale).max(0.0)) * (std::f32::consts::SQRT_2 - 1.0) / std::f32::consts::SQRT_2).ceil() as i32 + 1;
        let r = Rect {
            left: rect.left + if attached.left { 0 } else { d },
            top: rect.top + if attached.top { 0 } else { d },
            right: rect.right - if attached.right { 0 } else { d },
            bottom: rect.bottom - if attached.bottom { 0 } else { d },
        };
        if r.width() < 4 || r.height() < 4 {
            return self.hide();
        }
        if self.last.get() == Some(r) {
            return;
        }
        self.last.set(Some(r));
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(panel), r.left, r.top, r.width(), r.height(), SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    pub fn hide(&self) {
        if self.last.take().is_some() {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }
}

impl Drop for BlurPane {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
