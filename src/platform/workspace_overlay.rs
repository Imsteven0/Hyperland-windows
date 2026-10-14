//! The stage for workspace switch animations: a borderless topmost window
//! with no surface of its own (`WS_EX_NOREDIRECTIONBITMAP`) that only shows
//! DWM thumbnails — the live desktop (wallpaper and icons, Progman) as a
//! still backdrop, and the windows of both workspaces, which the caller moves
//! frame by frame. Thumbnails are composed by DWM on the GPU, keep updating
//! live, and also work for windows komorebi has already cloaked.

use std::{collections::HashMap, ffi::c_void, mem::size_of, sync::Once};

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Dwm::{
            DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE, DWMWA_EXCLUDED_FROM_PEEK,
            DWMWA_TRANSITIONS_FORCEDISABLED, DwmFlush, DwmRegisterThumbnail, DwmSetWindowAttribute, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, EnumWindows, FindWindowW, GetWindowRect, HWND_TOPMOST, IsIconic, IsWindow, MA_NOACTIVATE, RegisterClassW, SWP_HIDEWINDOW,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos, WM_MOUSEACTIVATE, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP,
            WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        },
    },
    core::{BOOL, PCWSTR, w},
};

use super::window::Rect;

/// Which workspace a thumbnail belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// The workspace being left.
    Old,
    /// The workspace being entered.
    New,
    /// Stays put on top (windows shown on every workspace, e.g. Settings).
    Pinned,
}

/// One window's thumbnail on the stage. `Send`: the animation runs on a worker.
#[derive(Debug, Clone, Copy)]
pub struct Sprite {
    thumb: isize,
    /// Resting place in stage coordinates (where the window really is).
    base: Rect,
    pub layer: Layer,
}

impl Sprite {
    /// Draws the window shifted by `offset`, at `alpha`.
    pub fn place(&self, (dx, dy): (i32, i32), alpha: u8) {
        let props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_OPACITY | DWM_TNP_VISIBLE,
            rcDestination: RECT { left: self.base.left + dx, top: self.base.top + dy, right: self.base.right + dx, bottom: self.base.bottom + dy },
            opacity: alpha,
            fVisible: (alpha > 0).into(),
            ..Default::default()
        };
        // Fails harmlessly if the stage was torn down meanwhile.
        let _ = unsafe { DwmUpdateThumbnailProperties(self.thumb, &props) };
    }
}

/// Blocks until DWM has composed the next frame: paces the animation to the
/// display and makes sure the stage is on screen before windows change.
pub fn wait_for_compositor() {
    let _ = unsafe { DwmFlush() };
}

unsafe extern "system" fn stage_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

fn stage_class() -> PCWSTR {
    static REGISTER: Once = Once::new();
    let name = w!("CaelestiaWorkspaceStage");
    REGISTER.call_once(|| unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(stage_proc),
            hInstance: GetModuleHandleW(None).map(Into::into).unwrap_or_default(),
            lpszClassName: name,
            ..Default::default()
        };
        RegisterClassW(&wc);
    });
    name
}

pub struct Overlay {
    hwnd: HWND,
    /// Stage rectangle on screen.
    area: Rect,
    thumbs: Vec<isize>,
    sprites: Vec<Sprite>,
    /// Windows mirrored as `Layer::Pinned`.
    pinned_sources: Vec<isize>,
}

impl Overlay {
    pub fn new() -> Option<Self> {
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP,
                stage_class(),
                w!("caelestia-workspace-stage"),
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
            let on: BOOL = true.into();
            for attr in [DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_EXCLUDED_FROM_PEEK] {
                let _ = DwmSetWindowAttribute(hwnd, attr, &on as *const _ as *const c_void, size_of::<BOOL>() as u32);
            }
            // Covers a whole monitor while it plays: not a full-screen app.
            super::window::not_fullscreen(hwnd);
            Some(Self { hwnd, area: Rect::default(), thumbs: Vec::new(), sprites: Vec::new(), pinned_sources: Vec::new() })
        }
    }

    pub fn size(&self) -> (i32, i32) {
        (self.area.width(), self.area.height())
    }

    /// Covers `area` with the desktop and `windows` exactly where they are,
    /// so the switch underneath can't be seen; `pinned` windows are mirrored
    /// on top and `keep_above` (the shell's bar and frame) stay above the
    /// stage, like Hyprland's layers. Returns false (and shows nothing) when
    /// the desktop can't be mirrored.
    pub fn cover(&mut self, area: Rect, windows: &[isize], pinned: &[isize], keep_above: &[HWND]) -> bool {
        self.release();
        self.area = area;
        self.pinned_sources = pinned.to_vec();
        unsafe {
            let Ok(desktop) = FindWindowW(w!("Progman"), None) else { return false };
            let mut d = RECT::default();
            if GetWindowRect(desktop, &mut d).is_err() {
                return false;
            }
            let Ok(thumb) = DwmRegisterThumbnail(self.hwnd, desktop) else { return false };
            self.thumbs.push(thumb);
            // Progman spans the virtual screen: show this monitor's part, still.
            let props = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_RECTSOURCE | DWM_TNP_OPACITY | DWM_TNP_VISIBLE | DWM_TNP_SOURCECLIENTAREAONLY,
                rcDestination: RECT { left: 0, top: 0, right: area.width(), bottom: area.height() },
                rcSource: RECT { left: area.left - d.left, top: area.top - d.top, right: area.right - d.left, bottom: area.bottom - d.top },
                opacity: 255,
                fVisible: true.into(),
                fSourceClientAreaOnly: false.into(),
            };
            if DwmUpdateThumbnailProperties(thumb, &props).is_err() {
                self.release();
                return false;
            }
            self.add(windows, Layer::Old, true);
            self.add(pinned, Layer::Pinned, true);
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), area.left, area.top, area.width(), area.height(), SWP_NOACTIVATE | SWP_SHOWWINDOW);
            for &h in keep_above {
                let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
            }
        }
        wait_for_compositor();
        true
    }

    /// Adds the windows of the workspace being entered, hidden until the
    /// first animation frame places them, below the pinned ones.
    pub fn add_incoming(&mut self, windows: &[isize]) {
        let pinned: Vec<Sprite> = self.sprites.iter().filter(|s| s.layer == Layer::Pinned).copied().collect();
        self.add(windows, Layer::New, false);
        // DWM draws later thumbnails on top: re-register pinned ones last.
        for p in pinned {
            if let Some(i) = self.sprites.iter().position(|s| s.thumb == p.thumb) {
                self.sprites.remove(i);
            }
            self.thumbs.retain(|&t| t != p.thumb);
            unsafe {
                let _ = DwmUnregisterThumbnail(p.thumb);
            }
        }
        let windows: Vec<isize> = self.pinned_sources.clone();
        self.add(&windows, Layer::Pinned, true);
    }

    /// Snapshot of the window thumbnails for the animation thread.
    pub fn sprites(&self) -> Vec<Sprite> {
        self.sprites.clone()
    }

    /// Hides the stage (the real windows are already in place underneath)
    /// and drops every thumbnail.
    pub fn release(&mut self) {
        unsafe {
            if !self.thumbs.is_empty() {
                let _ = SetWindowPos(self.hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_HIDEWINDOW);
            }
            for t in self.thumbs.drain(..) {
                let _ = DwmUnregisterThumbnail(t);
            }
        }
        self.sprites.clear();
    }

    fn add(&mut self, windows: &[isize], layer: Layer, visible: bool) {
        // Bottom-most first: later thumbnails are drawn on top.
        let order = z_order();
        let mut windows: Vec<isize> = windows.to_vec();
        windows.sort_by_key(|h| std::cmp::Reverse(order.get(h).copied().unwrap_or(usize::MAX)));
        for h in windows {
            let hwnd = HWND(h as *mut c_void);
            unsafe {
                if !IsWindow(Some(hwnd)).as_bool() || IsIconic(hwnd).as_bool() {
                    continue;
                }
                let mut r = RECT::default();
                if GetWindowRect(hwnd, &mut r).is_err() {
                    continue;
                }
                let a = &self.area;
                if r.right <= a.left || r.left >= a.right || r.bottom <= a.top || r.top >= a.bottom {
                    continue; // on another monitor
                }
                let Ok(thumb) = DwmRegisterThumbnail(self.hwnd, hwnd) else { continue };
                self.thumbs.push(thumb);
                let sprite = Sprite { thumb, base: Rect { left: r.left - a.left, top: r.top - a.top, right: r.right - a.left, bottom: r.bottom - a.top }, layer };
                sprite.place((0, 0), if visible { 255 } else { 0 });
                self.sprites.push(sprite);
            }
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.release();
    }
}

/// Top-level windows by z-order (0 = topmost).
fn z_order() -> HashMap<isize, usize> {
    unsafe extern "system" fn each(hwnd: HWND, l: LPARAM) -> BOOL {
        let map = unsafe { &mut *(l.0 as *mut HashMap<isize, usize>) };
        let n = map.len();
        map.insert(hwnd.0 as isize, n);
        BOOL(1)
    }
    let mut map = HashMap::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut map as *mut _ as isize));
    }
    map
}
