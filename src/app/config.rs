//! Config and theme: pushing shell.json and scheme.json into every window, hotkeys, saving settings.

use super::*;

impl Caelestia {
    pub(super) fn bar_width(&self) -> f32 {
        self.cfg.borrow().f32("bar.sizes.innerWidth").max(24.0) + 8.0
    }

    pub(super) fn apply_config(&self) {
        let lang = crate::i18n::resolve(&self.cfg.borrow().str("general.language"), &system_lang());
        if self.st.borrow().lang != lang {
            crate::i18n::set_language(&lang);
            self.st.borrow_mut().lang = lang;
            // Strings built in Rust must be rebuilt in the new language.
            let weak = self.weak.clone();
            Timer::single_shot(Duration::ZERO, move || {
                if let Some(app) = weak.upgrade() {
                    app.set_static_info();
                    app.refresh_recordings();
                    app.tick_clock_force();
                    let q = app.launcher.get_query();
                    app.search(&q);
                }
            });
        }
        let cfg = self.cfg.borrow();
        let scheme = self.scheme.borrow();
        let palette = scheme.palette();
        let tokens = Tokens {
            rounding_scale: cfg.f32("appearance.rounding.scale"),
            spacing_scale: cfg.f32("appearance.spacing.scale"),
            padding_scale: cfg.f32("appearance.padding.scale"),
            font_scale: cfg.f32("appearance.font.size.scale"),
            anim_scale: self.anim_scale(),
            transparency: cfg.bool("appearance.transparency.enabled"),
            base_alpha: cfg.f32("appearance.transparency.base"),
            layer_alpha: cfg.f32("appearance.transparency.layers"),
            font_sans: cfg.str("appearance.font.family.sans").into(),
            font_mono: cfg.str("appearance.font.family.mono").into(),
            font_material: cfg.str("appearance.font.family.material").into(),
            font_clock: cfg.str("appearance.font.family.clock").into(),
            border_thickness: cfg.f32("border.thickness"),
            border_rounding: cfg.f32("border.rounding"),
            bar_width: cfg.f32("bar.sizes.innerWidth").max(24.0) + 8.0,
        };
        let ui = UiConfig {
            bar_persistent: cfg.bool("bar.persistent"),
            bar_show_workspaces: cfg.bar_entry("workspaces"),
            bar_workspaces_shown: cfg.i64("bar.workspaces.shown").clamp(1, 10) as i32,
            bar_active_trail: cfg.bool("bar.workspaces.activeTrail"),
            bar_show_active_window: cfg.bar_entry("activeWindow"),
            bar_show_clock: cfg.bar_entry("clock"),
            bar_show_status: cfg.bar_entry("statusIcons"),
            bar_show_power: cfg.bar_entry("power"),
            bar_show_audio: cfg.bool("bar.status.showAudio"),
            bar_show_network: cfg.bool("bar.status.showNetwork"),
            bar_show_bluetooth: cfg.bool("bar.status.showBluetooth"),
            bar_show_battery: cfg.bool("bar.status.showBattery"),
            bar_show_notifications: cfg.bool("bar.status.showNotifications"),
            clock_12h: cfg.bool("services.useTwelveHourClock"),
            dashboard_enabled: cfg.bool("dashboard.enabled"),
            dashboard_show_on_hover: cfg.bool("dashboard.showOnHover"),
            launcher_max_shown: cfg.i64("launcher.maxShown").clamp(1, 20) as i32,
            launcher_action_prefix: cfg.str("launcher.actionPrefix").into(),
            clipboard_history: cfg.bool("launcher.clipboardHistory"),
            clipboard_paste: cfg.bool("launcher.clipboardPaste"),
            use_fahrenheit: cfg.bool("services.useFahrenheit"),
            weather_location: cfg.str("services.weatherLocation").into(),
            weather_location_name: cfg.str("services.weatherLocationName").into(),
            wallpaper_enabled: cfg.bool("background.enabled"),
            wallpaper_path: self.st.borrow().wallpaper.as_ref().map(|p| p.display().to_string()).unwrap_or_default().into(),
            scheme_variant: scheme.variant.clone().into(),
            scheme_mode: scheme.mode.clone().into(),
            scheme_name: scheme.name.clone().into(),
            smart_scheme: cfg.bool("services.smartScheme"),
            anim_scale: self.anim_scale(),
            blur: cfg.bool("windows.blur"),
            super_launcher: cfg.str("windows.shortcuts.launcher").eq_ignore_ascii_case("super"),
            wallpaper_repo: cfg.str("paths.wallpaperRepo").into(),
            autostart: cfg.bool("windows.autostart"),
            komorebi_enabled: cfg.bool("windows.komorebi.enabled"),
            komorebi_autostart: cfg.bool("windows.komorebi.autostart"),
            komorebi_borders: cfg.bool("windows.komorebi.borders"),
            komorebi_gap_inner: cfg.f32("windows.komorebi.containerPadding"),
            komorebi_gap_outer: cfg.f32("windows.komorebi.workspacePadding"),
            komorebi_border_width: cfg.f32("windows.komorebi.borderWidth"),
            komorebi_animations: cfg.bool("windows.komorebi.animations"),
            komorebi_ws_anim: cfg.str("windows.komorebi.workspaceAnimation").into(),
            language: cfg.str("general.language").into(),
            border_thickness: cfg.f32("border.thickness"),
            border_rounding: cfg.f32("border.rounding"),
            rounding_scale: cfg.f32("appearance.rounding.scale"),
            font_scale: cfg.f32("appearance.font.size.scale"),
            transparency: tokens.transparency,
            base_alpha: tokens.base_alpha,
            layer_alpha: tokens.layer_alpha,
            config_path: cfg.paths.shell_json.display().to_string().into(),
            perf_mode: cfg.str("windows.performance.mode").into(),
            perf_on_battery: cfg.bool("windows.performance.onBattery"),
            perf_on_fullscreen: cfg.bool("windows.performance.onFullscreen"),
            perf_active: self.perf_reason().map(PerfReason::id).unwrap_or_default().into(),
        };
        let dark = scheme.is_dark();
        each_window!(self, w => {
            let t = w.global::<Theme>();
            t.set_c(palette.clone());
            t.set_t(tokens.clone());
            t.set_dark(dark);
            w.global::<ShellGlobal>().set_cfg(ui.clone());
        });
        drop((cfg, scheme));

        // Things that depend on config but live outside Slint.
        if let Some(svc) = self.svc.borrow().as_ref() {
            let cfg = self.cfg.borrow();
            svc.sysmon.interval_ms.store(cfg.i64("dashboard.resourceUpdateInterval").max(250) as u64, Ordering::Relaxed);
            let q = weather_query(&cfg);
            let mut st = self.st.borrow_mut();
            if st.weather_query.as_ref() != Some(&q) {
                st.weather_query = Some(q.clone());
                svc.weather.update(q);
            }
        }
        if self.screens.borrow().iter().all(|s| s.styled) {
            self.layout_screens();
        }
        self.bind_hotkeys();
        crate::platform::autostart::sync(self.cfg.borrow().bool("windows.autostart"));
        self.apply_settings_backdrop();
        self.refresh_scheme_cards();
        self.ensure_blur_tracking();
        self.sync_clipboard_watcher();
        if !matches!(self.st.borrow().komorebi, KStatus::NotInstalled) && self.st.borrow().picker.is_none() {
            self.schedule_komorebi_theme();
        }
        self.tick_clock_force();
    }

    /// komorebi gets a setting once it rests for a moment; while komorebic
    /// runs, newer values also merge in its queue (`komorebi::run_in_order`).
    fn schedule_komorebi_theme(&self) {
        let weak = self.weak.clone();
        self.komorebi_theme_timer.start(TimerMode::SingleShot, Duration::from_millis(40), move || {
            if let Some(app) = weak.upgrade() {
                app.push_komorebi_theme();
            }
        });
    }

    /// Refreshes the UI from the config once per frame, however many
    /// settings changed in between.
    fn schedule_apply_config(&self) {
        if self.config_apply_timer.running() {
            return;
        }
        let weak = self.weak.clone();
        self.config_apply_timer.start(TimerMode::SingleShot, Duration::from_millis(16), move || {
            if let Some(app) = weak.upgrade() {
                app.apply_config();
            }
        });
    }

    /// Writes shell.json shortly after the last change.
    fn schedule_config_save(&self) {
        let weak = self.weak.clone();
        self.config_save_timer.start(TimerMode::SingleShot, Duration::from_millis(400), move || {
            if let Some(app) = weak.upgrade() {
                app.flush_config_save();
            }
        });
    }

    /// Saves now if a change is still waiting (also on exit).
    pub(super) fn flush_config_save(&self) {
        if !self.config_save_timer.running() && !self.st.borrow().config_unsaved {
            return;
        }
        self.config_save_timer.stop();
        self.st.borrow_mut().config_unsaved = false;
        if let Err(e) = self.cfg.borrow_mut().save() {
            log::error!("saving config: {e:#}");
        }
    }

    pub(super) fn bind_hotkeys(&self) {
        let binds: Vec<(String, String)> = self
            .cfg
            .borrow()
            .get("windows.shortcuts")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned()))).collect())
            .unwrap_or_default();
        let mut binds = binds;
        if self.cfg.borrow().bool("windows.komorebi.enabled") {
            // `windows.komorebi.binds`: { "Alt+H": "focus left", ... } overrides the defaults.
            let custom: Vec<(String, String)> = self
                .cfg
                .borrow()
                .get("windows.komorebi.binds")
                .and_then(Value::as_object)
                .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|c| (format!("komorebi:{c}"), k.clone()))).collect())
                .unwrap_or_default();
            if custom.is_empty() {
                binds.extend(komorebi::default_binds().into_iter().map(|(accel, cmd)| (format!("komorebi:{cmd}"), accel.to_owned())));
            } else {
                binds.extend(custom);
            }
        }
        if let Some(hk) = self.hotkeys.borrow_mut().as_mut() {
            let _ = hk.bind(&binds);
        }
    }

    pub(super) fn set_config(&self, path: &str, json: &str) {
        let value = ui_value(json);
        match path {
            "@wifi" => return self.radio(RadioCmd::ToggleWifi),
            "@bluetooth" => return self.radio(RadioCmd::ToggleBluetooth),
            "@mute" => {
                audio::set_muted(false, value.as_bool().unwrap_or(false));
                return self.refresh_status();
            }
            "@mic-mute" => {
                audio::set_muted(true, value.as_bool().unwrap_or(false));
                return self.refresh_status();
            }
            "services.scheme.dark" => return self.set_scheme(None, Some(value.as_bool().unwrap_or(true))),
            "services.scheme.variant" => return self.set_scheme(value.as_str().map(str::to_owned), None),
            _ => {}
        }
        let value = normalize_number(path, value);
        if let Err(e) = self.cfg.borrow_mut().set(path, value) {
            log::error!("setting {path}: {e:#}");
            return;
        }
        self.st.borrow_mut().config_unsaved = true;
        self.schedule_config_save();
        // These act on the applied config right away; the rest (sliders
        // included) refresh once per frame.
        if !(path.starts_with("windows.performance.") || path == "windows.komorebi.enabled") {
            return self.schedule_apply_config();
        }
        self.apply_config();
        if path.starts_with("windows.performance.") {
            self.refresh_perf();
        }
        if path == "windows.komorebi.enabled" {
            let on = self.cfg.borrow().bool("windows.komorebi.enabled");
            let status = self.st.borrow().komorebi.clone();
            self.st.borrow_mut().desktops = Desktops::default();
            self.st.borrow_mut().komorebi_visuals = None;
            match status {
                KStatus::Stopped if on => {
                    self.push_komorebi_theme();
                    komorebi::start();
                }
                KStatus::Running(state) if on => {
                    self.apply_komorebi_workspaces(&state);
                    self.push_komorebi_theme();
                }
                _ => self.refresh_workspaces(),
            }
        }
    }

    pub(super) fn on_file_changed(&self, which: Changed) {
        match which {
            Changed::Shell => {
                let reloaded = self.cfg.borrow_mut().reload();
                match reloaded {
                    Ok(true) => {
                        log::info!("shell.json changed, reloading");
                        self.apply_config();
                    }
                    Ok(false) => {}
                    Err(e) => log::error!("invalid shell.json, keeping previous config: {e:#}"),
                }
            }
            Changed::Scheme => {
                let path = self.cfg.borrow().paths.scheme_json.clone();
                match Scheme::load(&path) {
                    Ok(s) => {
                        *self.scheme.borrow_mut() = s;
                        self.apply_config();
                    }
                    Err(e) => log::error!("invalid scheme.json: {e:#}"),
                }
            }
        }
    }

    /// Regenerates the dynamic scheme (variant and/or mode) and persists it.
    pub(super) fn set_scheme(&self, variant: Option<String>, dark: Option<bool>) {
        let (variant, dark, dynamic) = {
            let s = self.scheme.borrow();
            (variant.unwrap_or_else(|| s.variant.clone()), dark.unwrap_or(s.is_dark()), s.is_dynamic())
        };
        if !dynamic {
            let (name, flavour) = {
                let s = self.scheme.borrow();
                (s.name.clone(), s.flavour.clone())
            };
            let same = crate::schemes::find(&name, &flavour).filter(|p| p.dark == dark);
            let sibling = same.or_else(|| crate::schemes::PRESETS.iter().find(|p| p.name == name && p.dark == dark));
            match sibling {
                Some(p) => *self.scheme.borrow_mut() = crate::schemes::to_scheme(p),
                None => self.scheme.borrow_mut().mode = if dark { "dark" } else { "light" }.into(),
            }
        } else {
            let wallpaper = self.st.borrow().wallpaper.clone();
            let paths = self.cfg.borrow().paths.clone();
            std::thread::spawn(move || {
                let source = wallpaper.and_then(|p| source_from_image(&p).ok()).unwrap_or(material_colors::color::Rgb::from_u32(0x4a6fa5));
                let scheme = Scheme::generate(source, &variant, dark);
                if let Err(e) = scheme.save(&paths.scheme_json) {
                    log::error!("saving scheme: {e:#}");
                }
                post(move |app| {
                    *app.scheme.borrow_mut() = scheme;
                    app.apply_config();
                });
            });
            return;
        }
        let s = self.scheme.borrow().clone();
        let _ = s.save(&self.cfg.borrow().paths.scheme_json);
        self.apply_config();
    }
}
