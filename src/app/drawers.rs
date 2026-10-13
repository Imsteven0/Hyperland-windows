//! Drawers: placing, opening and closing the dashboard, launcher, utilities, session and sidebar, plus hover-to-open.

use super::*;

impl Caelestia {
    pub(super) fn layout_screens(&self) {
        let (bar_l, t_l, r_l, persistent, blur) = {
            let cfg = self.cfg.borrow();
            let blur = cfg.bool("appearance.transparency.enabled") && cfg.bool("windows.blur") && self.perf_reason().is_none();
            (self.bar_width(), cfg.f32("border.thickness"), cfg.f32("border.rounding"), cfg.bool("bar.persistent"), blur)
        };
        for s in self.screens.borrow_mut().iter_mut() {
            s.area = win::shell_area(&s.monitor);
            let (area, sc) = (s.area, s.monitor.scale);
            let bar_w = (bar_l * sc).round() as i32;
            let t = (t_l * sc).round() as i32;
            s.border.window().set_position(PhysicalPosition::new(area.left, area.top));
            s.border.window().set_size(slint::PhysicalSize::new(area.width() as u32, area.height() as u32));
            s.bar.window().set_position(PhysicalPosition::new(area.left, area.top));
            s.bar.window().set_size(slint::PhysicalSize::new(bar_w as u32, area.height() as u32));
            if let Some(hwnd) = win::hwnd_of(s.border.window()) {
                win::set_frame_region(hwnd, area.width(), area.height(), bar_w, t, (r_l * sc).round() as i32);
                // DWM blur ignores window regions: never blur the full-screen frame.
                win::set_blur_behind(hwnd, false);
            }
            if let Some(hwnd) = win::hwnd_of(s.bar.window()) {
                win::set_blur_behind(hwnd, blur);
            }
            if persistent {
                if s.appbars.is_empty() {
                    s.appbars = [AbEdge::Left, AbEdge::Top, AbEdge::Right, AbEdge::Bottom].into_iter().filter_map(AppBar::new).collect();
                }
                for b in &s.appbars {
                    b.reserve(&area, if b.edge() == AbEdge::Left { bar_w } else { t });
                }
            } else {
                s.appbars.clear();
            }
        }
    }

    pub(super) fn style_surface(&self, window: &slint::Window, style: SurfaceStyle) {
        if let Some(hwnd) = win::hwnd_of(window) {
            win::make_surface(hwnd, style);
        }
    }

    pub(super) fn drawer_window(&self, d: Drawer) -> &slint::Window {
        match d {
            Drawer::Dashboard => self.dashboard.window(),
            Drawer::Launcher => self.launcher.window(),
            Drawer::Utilities => self.utilities.window(),
            Drawer::Session => self.session.window(),
            Drawer::Sidebar => self.sidebar.window(),
        }
    }

    pub(super) fn set_drawer_flag(&self, d: Drawer, open: bool) {
        match d {
            Drawer::Dashboard => self.dashboard.set_open(open),
            Drawer::Launcher => self.launcher.set_open(open),
            Drawer::Utilities => self.utilities.set_open(open),
            Drawer::Session => self.session.set_open(open),
            Drawer::Sidebar => self.sidebar.set_open(open),
        }
    }

    /// Places a drawer against its screen edge, flush with the border.
    pub(super) fn position_drawer(&self, d: Drawer) -> Rect {
        let window = self.drawer_window(d);
        let size = window.size();
        let (w, h) = (size.width as i32, size.height as i32);
        let screen = self.st.borrow().drawers[d as usize].screen;
        let (area, s) = self.screen_geom(screen);
        let bar_w = (self.bar_width() * s).round() as i32;
        let t = (self.cfg.borrow().f32("border.thickness") * s).round() as i32;
        let inner_left = area.left + bar_w;
        let inner_w = area.width() - bar_w - t;
        let (x, y) = match d {
            Drawer::Dashboard => (inner_left + (inner_w - w) / 2, area.top + t),
            Drawer::Launcher => (inner_left + (inner_w - w) / 2, area.bottom - t - h),
            Drawer::Utilities => (area.right - t - w, area.bottom - t - h),
            Drawer::Session => (area.right - t - w, area.top + (area.height() - h) / 2),
            // Anchored at the top: it grows downwards as notifications arrive.
            Drawer::Sidebar => (area.right - t - w, area.top + t),
        };
        window.set_position(PhysicalPosition::new(x, y));
        Rect { left: x, top: y, right: x + w, bottom: y + h }
    }

    pub fn toggle(&self, d: Drawer) {
        if self.st.borrow().drawers[d as usize].open {
            self.close(d);
        } else {
            self.open(d, false);
        }
    }

    pub fn open(&self, d: Drawer, by_hover: bool) {
        if d == Drawer::Dashboard && !self.cfg.borrow().bool("dashboard.enabled") {
            return;
        }
        if !by_hover {
            // A deliberate open replaces any other drawer (except a hover-peeked one).
            for other in Drawer::ALL {
                if other != d && self.st.borrow().drawers[other as usize].open {
                    self.close(other);
                }
            }
        }
        let screen = self.active_screen();
        self.st.borrow_mut().drawers[d as usize].screen = screen;
        let window = self.drawer_window(d);
        if let Err(e) = window.show() {
            log::error!("show drawer: {e}");
            return;
        }
        let interactive = matches!(d, Drawer::Launcher | Drawer::Session);
        self.style_surface(window, SurfaceStyle { click_through: false, no_activate: !interactive, topmost: true });
        let rect = self.position_drawer(d);
        // Moving to a monitor with another DPI rescales the window: place it again.
        let weak = self.weak.clone();
        Timer::single_shot(Duration::from_millis(60), move || {
            if let Some(app) = weak.upgrade() {
                if app.st.borrow().drawers[d as usize].open {
                    let r = app.position_drawer(d);
                    app.st.borrow_mut().drawers[d as usize].rect = Some(r);
                }
            }
        });
        {
            let mut st = self.st.borrow_mut();
            let ds = &mut st.drawers[d as usize];
            ds.open = true;
            ds.by_hover = by_hover;
            ds.rect = Some(rect);
            ds.generation += 1;
        }
        match d {
            Drawer::Dashboard => {
                if let Some(svc) = self.svc.borrow().as_ref() {
                    svc.sysmon.set_active(true);
                }
            }
            Drawer::Launcher => {
                self.launcher.set_picker_mode("".into());
                self.launcher.set_wallpaper_mode(false);
                self.launcher.invoke_reset();
                if self.st.borrow().apps_indexed_at.is_none_or(|t| t.elapsed() > Duration::from_secs(300)) {
                    self.reindex_apps();
                }
                self.search("");
            }
            Drawer::Session => self.session.set_focused(1),
            Drawer::Utilities => self.refresh_recordings(),
            Drawer::Sidebar => self.push_notifications(),
        }
        self.set_drawer_flag(d, true);
        self.ensure_blur_tracking();
        if interactive {
            self.focus_drawer(d);
            // The native window may only become activatable a frame later
            // (first show / DPI move): retry briefly until it has focus.
            for delay in [40u64, 120, 300] {
                let weak = self.weak.clone();
                Timer::single_shot(Duration::from_millis(delay), move || {
                    if let Some(app) = weak.upgrade() {
                        if app.st.borrow().drawers[d as usize].open {
                            app.focus_drawer(d);
                        }
                    }
                });
            }
        }
    }

    /// Gives a keyboard-driven drawer OS focus and puts the caret where the
    /// user types (launcher search / session buttons).
    pub(super) fn focus_drawer(&self, d: Drawer) {
        let Some(hwnd) = win::hwnd_of(self.drawer_window(d)) else { return };
        if win::foreground_window() != hwnd {
            win::focus_window(hwnd);
        }
        match d {
            Drawer::Launcher => self.launcher.invoke_focus_search(),
            Drawer::Session => self.session.invoke_focus_keys(),
            _ => {}
        }
    }

    pub fn close(&self, d: Drawer) {
        let generation = {
            let mut st = self.st.borrow_mut();
            let ds = &mut st.drawers[d as usize];
            if !ds.open {
                return;
            }
            ds.open = false;
            ds.by_hover = false;
            ds.generation += 1;
            ds.generation
        };
        self.set_drawer_flag(d, false);
        if d == Drawer::Launcher {
            self.picker_cancel();
            self.wallpaper_preview_cancel();
        }
        if d == Drawer::Dashboard {
            if let Some(svc) = self.svc.borrow().as_ref() {
                svc.sysmon.set_active(false);
            }
        }
        // Hide once the close animation has finished, unless reopened meanwhile.
        let weak = self.weak.clone();
        Timer::single_shot(self.close_delay(), move || {
            if let Some(app) = weak.upgrade() {
                let ds = app.st.borrow().drawers[d as usize];
                if ds.generation == generation && !ds.open {
                    let _ = app.drawer_window(d).hide();
                }
            }
        });
    }

    /// How long a drawer/popout takes to close: `Theme.anim-spatial` (500 ms,
    /// scaled) plus a frame of slack, after which its window can be hidden.
    pub(super) fn close_delay(&self) -> Duration {
        Duration::from_millis((500.0 * self.anim_scale().max(0.1)) as u64 + 60)
    }

    pub(super) fn close_all(&self) {
        for d in Drawer::ALL {
            self.close(d);
        }
    }

    /// Hover-to-open for the dashboard (top edge) and utilities (bottom-right
    /// corner) on whichever screen the pointer is, plus popout dismissal.
    pub(super) fn poll_hover(&self) {
        // In a full-screen game nothing can open from the edges: with no
        // popout or hover-opened drawer to dismiss, there is nothing to track.
        if self.perf_reason() == Some(PerfReason::Fullscreen) {
            let st = self.st.borrow();
            if !st.popout.open && !st.drawers.iter().any(|d| d.open && d.by_hover) {
                return;
            }
        }
        let (x, y) = win::cursor_pos();
        self.poll_popout(x, y);
        let idx = self.active_screen();
        let (area, s) = self.screen_geom(idx);
        let drawers = self.st.borrow().drawers;
        let cfg = self.cfg.borrow();
        let t = (cfg.f32("border.thickness") * s).round() as i32;
        let edge = t.max(2) + 2;
        // A full-screen game must never get a drawer from the mouse touching an edge.
        let hover_ok = self.perf_reason() != Some(PerfReason::Fullscreen);
        let dash_on = hover_ok && cfg.bool("dashboard.enabled") && cfg.bool("dashboard.showOnHover");
        let util_on = hover_ok && cfg.bool("utilities.enabled") && cfg.bool("utilities.showOnHover");
        drop(cfg);

        let check = |d: Drawer, zone: Rect| {
            let ds = drawers[d as usize];
            if !ds.open && area.contains(x, y) && zone.contains(x, y) {
                self.open(d, true);
            } else if ds.open && ds.by_hover {
                let inside = ds.rect.map(|r| r.inflate((8.0 * s) as i32).contains(x, y)).unwrap_or(false);
                if !inside && !(ds.screen == idx && zone.contains(x, y)) {
                    self.close(d);
                }
            }
        };
        if dash_on {
            let w = drawers[0].rect.map(|r| r.width()).unwrap_or((940.0 * s) as i32);
            let cx = area.left + (area.width() + (self.bar_width() * s) as i32 - t) / 2;
            check(Drawer::Dashboard, Rect { left: cx - w / 2, right: cx + w / 2, top: area.top, bottom: area.top + edge });
        }
        if util_on {
            let w = drawers[2].rect.map(|r| r.width()).unwrap_or((330.0 * s) as i32);
            check(Drawer::Utilities, Rect { left: area.right - w, right: area.right, top: area.bottom - edge, bottom: area.bottom });
        }
    }

    pub(super) fn on_foreground(&self, hwnd: windows::Win32::Foundation::HWND) {
        // Another app took focus: dismiss click-opened drawers (like clicking outside a popup).
        if let Some(info) = desktops::window_info(hwnd) {
            for d in Drawer::ALL {
                let ds = self.st.borrow().drawers[d as usize];
                if ds.open && !ds.by_hover {
                    self.close(d);
                }
            }
            self.set_active_title(&info.title, &info.app);
        }
        self.refresh_workspaces();
    }
}
