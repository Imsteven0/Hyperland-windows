//! Bar popouts and the blur panes that follow every translucent surface.

use super::*;

impl Caelestia {
    /// `arg` is `<kind>:<logical y of the item centre within the bar>`.
    pub(super) fn show_popout(&self, arg: &str) {
        let Some((kind, y)) = arg.split_once(':') else { return };
        let y: f32 = parse_num(y).unwrap_or(0.0);
        let (area, sc) = self.screen_geom(self.active_screen());
        let was_open = self.st.borrow().popout.open;
        if kind == "network" && !was_open {
            self.radio(RadioCmd::WifiScan);
        }
        if kind == "audio" {
            self.push_audio_devices(true); // the panel height depends on the lists
        }
        self.popout.set_kind(kind.into());
        if self.popout.show().is_err() {
            return;
        }
        if let Some(h) = win::hwnd_of(self.popout.window()) {
            win::make_surface(h, SurfaceStyle { click_through: false, no_activate: true, topmost: true });
        }
        // The window has a fixed (max) size; the panel inside morphs and is
        // vertically centred, so we centre the window on the hovered item.
        let (w, h) = ((self.popout.get_want_width() * sc).round() as i32, (self.popout.get_want_height() * sc).round() as i32);
        let half_panel = (self.popout.invoke_panel_height_for(kind.into()) * sc / 2.0).round() as i32;
        let t = (self.cfg.borrow().f32("border.thickness") * sc).round() as i32;
        let x = area.left + (self.bar_width() * sc).round() as i32;
        let cy = (area.top + (y * sc).round() as i32).clamp(area.top + t + half_panel, (area.bottom - t - half_panel).max(area.top + t + half_panel));
        let top = cy - h / 2;
        self.popout.window().set_size(slint::PhysicalSize::new(w as u32, h as u32));
        self.move_popout(x, top, was_open);
        {
            let mut st = self.st.borrow_mut();
            let p = &mut st.popout;
            p.open = true;
            p.kind = kind.to_owned();
            p.item_hovered = true;
            p.rect = Some(Rect { left: x, top: cy - half_panel, right: x + w, bottom: cy + half_panel });
            p.generation += 1;
        }
        if !was_open {
            self.popout.set_open(true);
        }
        self.ensure_blur_tracking();
    }

    /// Places the popout at `(x, top)`: a jump when it just opened (or in
    /// performance mode), else a spring glide from where it is, stretching
    /// along the motion like Caelestia's blobs. The timer runs only while moving.
    fn move_popout(&self, x: i32, top: i32, glide: bool) {
        let mut m = self.popout_motion.borrow_mut();
        m.x = x;
        m.target = top as f32;
        if !glide || self.anim_scale() <= 0.0 {
            (m.y, m.vel, m.last) = (top as f32, 0.0, None);
            drop(m);
            self.popout_timer.stop();
            self.popout.set_stretch(1.0);
            self.popout.window().set_position(PhysicalPosition::new(x, top));
            return;
        }
        if self.popout_timer.running() {
            return; // already gliding: it just heads for the new target
        }
        m.last = Some(Instant::now());
        drop(m);
        let weak = self.weak.clone();
        self.popout_timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
            if let Some(app) = weak.upgrade() {
                app.step_popout();
            }
        });
    }

    fn step_popout(&self) {
        let speed = self.anim_scale().max(0.1); // >1 = slower animations
        let (x, y, vel, settled) = {
            let mut m = self.popout_motion.borrow_mut();
            let now = Instant::now();
            // Real elapsed time (capped, so a stalled frame can't fling it).
            let dt = m.last.map_or(1.0 / 60.0, |t| now.duration_since(t).as_secs_f32()).min(1.0 / 30.0) / speed;
            m.last = Some(now);
            (m.y, m.vel) = spring_step(m.y, m.vel, m.target, dt);
            let settled = (m.y - m.target).abs() < 0.5 && m.vel.abs() < 5.0;
            if settled {
                (m.y, m.vel) = (m.target, 0.0);
            }
            (m.x, m.y, m.vel, settled)
        };
        self.popout.window().set_position(PhysicalPosition::new(x, y.round() as i32));
        // Stretch along the motion from the logical speed (px/s), at most 15%.
        let logical = vel.abs() / self.popout.window().scale_factor();
        self.popout.set_stretch(1.0 + (logical / 12_000.0).min(0.15));
        if settled {
            self.popout_timer.stop();
        }
    }

    /// Runs the blur-follow loop while any translucent surface is on screen.
    pub(super) fn ensure_blur_tracking(&self) {
        if self.blur_timer.running() {
            return;
        }
        let weak = self.weak.clone();
        self.blur_timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
            if let Some(app) = weak.upgrade() {
                app.update_blur();
            }
        });
    }

    pub(super) fn update_blur(&self) {
        use crate::platform::blur::{Attached, BlurPane};
        let (enabled, border_r, settings_r) = {
            let cfg = self.cfg.borrow();
            (
                cfg.bool("appearance.transparency.enabled") && cfg.bool("windows.blur") && self.perf_reason().is_none(),
                cfg.f32("border.rounding"),
                25.0 * cfg.f32("appearance.rounding.scale"),
            )
        };
        let a = |left, top, right, bottom| Attached { left, top, right, bottom };
        let geo = |x: f32, y: f32, w: f32, h: f32| (x, y, w, h);
        let surfaces: [(&slint::Window, (f32, f32, f32, f32), Attached, f32); 7] = [
            (self.dashboard.window(), geo(self.dashboard.get_blur_x(), self.dashboard.get_blur_y(), self.dashboard.get_blur_w(), self.dashboard.get_blur_h()), a(false, true, false, false), border_r),
            (self.launcher.window(), geo(self.launcher.get_blur_x(), self.launcher.get_blur_y(), self.launcher.get_blur_w(), self.launcher.get_blur_h()), a(false, false, false, true), border_r),
            (self.utilities.window(), geo(self.utilities.get_blur_x(), self.utilities.get_blur_y(), self.utilities.get_blur_w(), self.utilities.get_blur_h()), a(false, false, true, true), border_r),
            (self.session.window(), geo(self.session.get_blur_x(), self.session.get_blur_y(), self.session.get_blur_w(), self.session.get_blur_h()), a(false, false, true, false), border_r),
            (self.popout.window(), geo(self.popout.get_blur_x(), self.popout.get_blur_y(), self.popout.get_blur_w(), self.popout.get_blur_h()), a(true, false, false, false), border_r),
            (self.settings.window(), geo(0.0, 0.0, 885.0, 625.0), a(false, false, false, false), settings_r),
            (self.sidebar.window(), geo(self.sidebar.get_blur_x(), self.sidebar.get_blur_y(), self.sidebar.get_blur_w(), self.sidebar.get_blur_h()), a(false, true, true, false), border_r),
        ];
        let mut any = false;
        let mut panes = self.blur_panes.borrow_mut();
        for (i, (window, (x, y, w, h), attached, radius)) in surfaces.into_iter().enumerate() {
            let hwnd = win::hwnd_of(window).filter(|h| unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(*h).as_bool() });
            let settings_hidden = i == 5 && !self.st.borrow().settings_visible;
            let Some(hwnd) = hwnd.filter(|_| enabled && !settings_hidden) else {
                if let Some(p) = panes[i].as_ref() {
                    p.hide();
                }
                continue;
            };
            any = true;
            if panes[i].is_none() {
                panes[i] = BlurPane::new();
            }
            let Some(pane) = panes[i].as_ref() else { continue };
            let sf = window.scale_factor();
            let pos = window.position();
            let rect = Rect {
                left: pos.x + (x * sf).round() as i32,
                top: pos.y + (y * sf).round() as i32,
                right: pos.x + ((x + w) * sf).round() as i32,
                bottom: pos.y + ((y + h) * sf).round() as i32,
            };
            pane.follow(hwnd, rect, attached, radius * sf, sf);
        }
        if !any {
            self.blur_timer.stop();
        }
    }

    pub(super) fn close_popout(&self) {
        let generation = {
            let mut st = self.st.borrow_mut();
            if !st.popout.open {
                return;
            }
            st.popout.open = false;
            st.popout.generation += 1;
            st.popout.generation
        };
        self.popout.set_open(false);
        self.popout_timer.stop();
        self.popout.set_stretch(1.0);
        let weak = self.weak.clone();
        Timer::single_shot(self.close_delay(), move || {
            if let Some(app) = weak.upgrade() {
                let p = app.st.borrow().popout.clone();
                if !p.open && p.generation == generation {
                    let _ = app.popout.hide();
                }
            }
        });
    }

    pub(super) fn poll_popout(&self, x: i32, y: i32) {
        let p = self.st.borrow().popout.clone();
        if !p.open || p.item_hovered {
            return;
        }
        let inside = p.rect.map(|r| r.inflate(8).contains(x, y)).unwrap_or(false);
        if !inside {
            self.close_popout();
        }
    }
}
