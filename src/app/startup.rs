//! Startup: per-monitor bars and frames, callback wiring, services, timers and the config watcher.

use super::*;

impl Caelestia {
    pub fn start(&self) -> Result<()> {
        self.sync_screens();
        self.start_services();
        self.start_timers();
        self.start_watcher();
        self.reindex_apps();

        let weak = self.weak.clone();
        desktops::watch_foreground(move |hwnd| {
            if let Some(app) = weak.upgrade() {
                app.on_foreground(hwnd);
            }
        });
        crate::ipc::listen(|cmd| post(move |app| app.on_ipc(cmd)));
        komorebi::spawn_watcher(|status| post(move |app| app.on_komorebi(status)));
        let weak = self.weak.clone();
        if let Some(hk) = Hotkeys::new(move |target| {
            if let Some(app) = weak.upgrade() {
                app.on_hotkey(&target);
            }
        }) {
            *self.hotkeys.borrow_mut() = Some(hk);
            self.bind_hotkeys();
        }
        self.refresh_workspaces();
        self.refresh_status();
        self.refresh_dnd();
        self.tick_clock();
        self.refresh_recordings();
        Ok(())
    }

    /// (Re)creates one bar + frame per monitor whenever the monitor layout
    /// changes (hot-plug, resolution, DPI, taskbar moves). Polled cheaply.
    pub(super) fn sync_screens(&self) {
        let excluded = self.cfg.borrow().str_list("bar.excludedScreens");
        let wanted: Vec<(Monitor, Rect)> = win::monitors()
            .into_iter()
            .filter(|m| !excluded.iter().any(|e| m.name.eq_ignore_ascii_case(e) || m.name.trim_start_matches(r"\\.\").eq_ignore_ascii_case(e)))
            .map(|m| {
                let area = win::shell_area(&m);
                (m, area)
            })
            .collect();
        let current: Vec<(Monitor, Rect)> = self.screens.borrow().iter().map(|s| (s.monitor.clone(), s.area)).collect();
        if current == wanted {
            return;
        }
        log::info!("monitor layout: {:?}", wanted.iter().map(|(m, _)| (&m.name, m.rect, m.scale)).collect::<Vec<_>>());
        for d in Drawer::ALL {
            self.close(d);
        }
        self.close_popout();
        let old = std::mem::take(&mut *self.screens.borrow_mut());
        for s in &old {
            let _ = s.bar.hide();
            let _ = s.border.hide();
        }
        drop(old); // releases the old AppBars

        let mut screens = Vec::new();
        for (idx, (monitor, area)) in wanted.into_iter().enumerate() {
            let (Ok(bar), Ok(border)) = (BarWindow::new(), BorderWindow::new()) else { continue };
            self.wire_global_for(bar.global::<ShellGlobal>(), Some(idx));
            self.wire_global(border.global::<ShellGlobal>());
            screens.push(Screen { monitor, area, bar, border, appbars: Vec::new(), styled: false });
        }
        *self.screens.borrow_mut() = screens;

        // New windows start with default globals: push the full current state.
        self.apply_config();
        each_window!(self, w => w.global::<ShellGlobal>().set_cookie_path(cookie_path().into()));
        self.st.borrow_mut().desktops = Desktops::default();
        self.refresh_workspaces();
        self.tick_clock_force();
        self.refresh_status();
        let (title, app) = self.st.borrow().active_title.clone();
        self.set_active_title(&title, &app);
        self.push_plugins();

        for s in self.screens.borrow().iter() {
            let _ = s.border.show();
            let _ = s.bar.show();
        }
        // winit creates native windows asynchronously; style and place each
        // screen once its HWNDs exist, then once more after DPI settles.
        let weak = self.weak.clone();
        self.startup_timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
            let Some(app) = weak.upgrade() else { return };
            let mut pending = false;
            for s in app.screens.borrow_mut().iter_mut().filter(|s| !s.styled) {
                if win::hwnd_of(s.border.window()).is_none() || win::hwnd_of(s.bar.window()).is_none() {
                    pending = true;
                    continue;
                }
                for w in [s.border.window(), s.bar.window()] {
                    if let Some(h) = win::hwnd_of(w) {
                        win::make_surface(h, SurfaceStyle { click_through: false, no_activate: true, topmost: true });
                    }
                }
                s.styled = true;
            }
            if !pending {
                app.startup_timer.stop();
                app.layout_screens();
                let weak = app.weak.clone();
                Timer::single_shot(Duration::from_millis(250), move || {
                    if let Some(app) = weak.upgrade() {
                        app.layout_screens();
                    }
                });
            }
        });
    }

    pub(super) fn wire_global(&self, g: ShellGlobal<'_>) {
        self.wire_global_for(g, None);
    }

    /// `screen` is set for per-monitor bars: workspace actions then target
    /// that monitor (komorebi has workspaces per monitor, like Hyprland).
    pub(super) fn wire_global_for(&self, g: ShellGlobal<'_>, screen: Option<usize>) {
        let weak = self.weak.clone();
        g.on_action(move |id, arg| {
            if let Some(app) = weak.upgrade() {
                match (id.as_str(), screen) {
                    ("workspace" | "workspace-relative", Some(idx)) if app.komorebi_active() => app.komorebi_workspace(idx, &id, &arg),
                    _ => app.action(&id, &arg),
                }
            }
        });
        let weak = self.weak.clone();
        g.on_set_config(move |path, json| {
            if let Some(app) = weak.upgrade() {
                app.set_config(&path, &json);
            }
        });
    }

    /// Screen under the pointer (drawers open where the user is).
    pub(super) fn active_screen(&self) -> usize {
        let (x, y) = win::cursor_pos();
        self.screens.borrow().iter().position(|s| s.monitor.rect.contains(x, y)).unwrap_or(0)
    }

    pub(super) fn screen_geom(&self, idx: usize) -> (Rect, f32) {
        let screens = self.screens.borrow();
        screens.get(idx).or(screens.first()).map(|s| (s.area, s.monitor.scale)).unwrap_or_else(|| {
            let m = win::primary_monitor();
            (win::shell_area(&m), m.scale)
        })
    }

    pub(super) fn primary_scale(&self) -> f32 {
        self.screens.borrow().first().map(|s| s.monitor.scale).unwrap_or(1.0)
    }

    pub(super) fn set_active_title(&self, title: &str, app: &str) {
        self.st.borrow_mut().active_title = (title.to_owned(), app.to_owned());
        for s in self.screens.borrow().iter() {
            let g = s.bar.global::<ShellGlobal>();
            g.set_active_title(title.into());
            g.set_active_app(app.into());
        }
        let g = self.popout.global::<ShellGlobal>();
        g.set_active_title(title.into());
        g.set_active_app(app.into());
    }

    pub(super) fn wire_callbacks(&self) {
        each_window!(self, w => self.wire_global(w.global::<ShellGlobal>()));

        let weak = self.weak.clone();
        self.launcher.on_search_changed(move |q| {
            if let Some(app) = weak.upgrade() {
                app.search(&q);
            }
        });
        let weak = self.weak.clone();
        self.launcher.on_activate(move |i| {
            if let Some(app) = weak.upgrade() {
                app.activate_hit(i as usize);
            }
        });
        let weak = self.weak.clone();
        self.launcher.on_activate_wallpaper(move |i| {
            if let Some(app) = weak.upgrade() {
                // Keep it: closing must not restore the original.
                app.st.borrow_mut().wallpaper_preview = None;
                let path = app.st.borrow().wallpapers.get(i as usize).cloned();
                if let Some(p) = path {
                    app.set_wallpaper(p);
                }
                app.close(Drawer::Launcher);
            }
        });
        let weak = self.weak.clone();
        self.launcher.on_preview_wallpaper(move |i| {
            if let Some(app) = weak.upgrade() {
                app.preview_wallpaper(i as usize);
            }
        });
        let weak = self.weak.clone();
        self.launcher.on_picker_preview(move |i| {
            if let Some(app) = weak.upgrade() {
                app.picker_preview(i as usize);
            }
        });
        let weak = self.weak.clone();
        self.launcher.on_picker_confirm(move |i| {
            if let Some(app) = weak.upgrade() {
                app.picker_confirm(i as usize);
            }
        });
        let weak = self.weak.clone();
        self.launcher.on_remove_entry(move |i| {
            if let Some(app) = weak.upgrade() {
                app.remove_launcher_entry(i as usize);
            }
        });
        let weak = self.weak.clone();
        self.launcher.on_dismiss(move || {
            if let Some(app) = weak.upgrade() {
                app.close(Drawer::Launcher);
            }
        });
        let weak = self.weak.clone();
        self.session.on_dismiss(move || {
            if let Some(app) = weak.upgrade() {
                app.close(Drawer::Session);
            }
        });

        // Launcher/session close when they lose keyboard focus, like a popup.
        use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};
        for d in [Drawer::Launcher, Drawer::Session] {
            let weak = self.weak.clone();
            let window = match d {
                Drawer::Launcher => self.launcher.window(),
                _ => self.session.window(),
            };
            window.on_winit_window_event(move |_, ev| {
                if let WindowEvent::Focused(false) = ev {
                    let weak = weak.clone();
                    // Defer: focus moves between our own windows during open.
                    Timer::single_shot(Duration::from_millis(120), move || {
                        if let Some(app) = weak.upgrade() {
                            let ours = desktops::window_info(win::foreground_window()).is_none();
                            if !ours {
                                app.close(d);
                            }
                        }
                    });
                }
                EventResult::Propagate
            });
        }

        // Settings window
        let weak = self.weak.clone();
        self.settings.on_close_requested(move || {
            if let Some(app) = weak.upgrade() {
                app.toggle_settings(Some(false));
            }
        });
        let weak = self.weak.clone();
        self.settings.on_drag(move || {
            if let Some(app) = weak.upgrade() {
                app.settings.window().with_winit_window(|w| {
                    let _ = w.drag_window();
                });
            }
        });
        let weak = self.weak.clone();
        self.settings.on_search(move |q| {
            if let Some(app) = weak.upgrade() {
                app.settings_search(&q);
            }
        });
    }

    pub(super) fn start_services(&self) {
        let cfg = self.cfg.borrow();
        let sysmon = SysMonitor::spawn(|snap| post(move |app| app.on_sys(snap)));
        sysmon.interval_ms.store(cfg.i64("dashboard.resourceUpdateInterval").max(250) as u64, Ordering::Relaxed);
        let media = Media::spawn(Duration::from_millis(cfg.i64("dashboard.mediaUpdateInterval").clamp(250, 5000) as u64), |snap| post(move |app| app.on_media(snap)));
        let q = weather_query(&cfg);
        self.st.borrow_mut().weather_query = Some(q.clone());
        let weather = WeatherService::spawn(q, |snap| post(move |app| app.on_weather(snap)));
        let net = Connectivity::spawn(|snap| post(move |app| app.on_net(snap)));
        let notifications = Notifications::spawn(|ev| post(move |app| app.on_notif_event(ev)));
        *self.svc.borrow_mut() = Some(Services { notifications, sysmon, media, weather, net });
        drop(cfg);
        *self.plugin_runner.borrow_mut() = Some(PluginRunner::spawn(|u| post(move |app| app.on_plugin_update(u))));
        self.load_plugins();
    }

    pub(super) fn start_timers(&self) {
        let mut timers = self.timers.borrow_mut();
        let every = |ms: u64, f: fn(&Caelestia)| {
            let t = Timer::default();
            let weak = self.weak.clone();
            t.start(TimerMode::Repeated, Duration::from_millis(ms), move || {
                if let Some(app) = weak.upgrade() {
                    f(&app);
                }
            });
            t
        };
        timers.push(every(1000, Caelestia::tick_clock));
        timers.push(every(1000, Caelestia::refresh_status));
        timers.push(every(1500, Caelestia::refresh_workspaces));
        timers.push(every(70, Caelestia::poll_hover));
        timers.push(every(30_000, Caelestia::refresh_recordings));
        timers.push(every(2_000, Caelestia::sync_screens));
        timers.push(every(5_000, Caelestia::refresh_dnd));
        timers.push(every(1_000, |app| {
            // Full-screen games: skip the window scan (process snapshot) entirely.
            if app.perf_reason() != Some(PerfReason::Fullscreen) {
                app.ghosts.borrow_mut().scan();
            }
        }));
        timers.push(every(2_000, Caelestia::refresh_perf));
    }

    pub(super) fn start_watcher(&self) {
        let paths = self.cfg.borrow().paths.clone();
        match Watcher::spawn(paths.shell_json, paths.scheme_json, |c| post(move |app| app.on_file_changed(c))) {
            Ok(w) => *self.watcher.borrow_mut() = Some(w),
            Err(e) => log::warn!("hot reload disabled: {e}"),
        }
    }
}
