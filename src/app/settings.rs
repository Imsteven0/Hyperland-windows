//! The Settings window: visibility, static info, schemes, opacity presets, search and dots export/import.

use super::*;

impl Caelestia {
    /// Settings › About: "Check now".
    pub(super) fn check_updates_now(&self) {
        self.settings.global::<ShellGlobal>().set_update_state("checking".into());
        match self.updates.borrow().as_ref() {
            Some(checker) => checker.check_now(),
            // Checks are off: still honour an explicit click, once.
            None => {
                std::thread::spawn(|| {
                    let c = crate::services::updates::check_once(env!("CARGO_PKG_VERSION"));
                    post(move |app| app.on_update_check(c));
                });
            }
        }
    }

    pub(super) fn on_update_check(&self, check: crate::services::updates::Check) {
        use crate::services::updates::Check;
        let g = self.settings.global::<ShellGlobal>();
        match check {
            Check::Available(r) => {
                log::info!("update available: {}", r.tag);
                g.set_update_state("available".into());
                g.set_update_version(r.tag.clone().into());
                self.st.borrow_mut().update = Some(r);
            }
            Check::UpToDate => {
                g.set_update_state("current".into());
                self.st.borrow_mut().update = None;
            }
            // Keep showing a known update if a later check fails (offline...).
            Check::Failed(_) if self.st.borrow().update.is_some() => g.set_update_state("available".into()),
            Check::Failed(_) => g.set_update_state("error".into()),
        }
    }

    pub(super) fn toggle_settings(&self, show: Option<bool>) {
        let visible = self.st.borrow().settings_visible;
        let want = show.unwrap_or(!visible);
        if want == visible {
            if want {
                if let Some(hwnd) = win::hwnd_of(self.settings.window()) {
                    win::focus_window(hwnd);
                }
            }
            return;
        }
        self.st.borrow_mut().settings_visible = want;
        if want {
            self.close_all();
            self.settings.set_shown(false);
            if self.settings.show().is_err() {
                self.st.borrow_mut().settings_visible = false;
                return;
            }
            let size = self.settings.window().size();
            let (a, _) = self.screen_geom(self.active_screen());
            self.settings.window().set_position(PhysicalPosition::new(a.left + (a.width() - size.width as i32) / 2, a.top + (a.height() - size.height as i32) / 2));
            self.apply_settings_backdrop();
            self.update_wallpaper_preview();
            self.refresh_settings_wallpapers();
            self.push_audio_devices(true);
            if self.settings.global::<ShellGlobal>().get_dots_state() != "fetching" && self.st.borrow().dots_preview.is_none() {
                self.set_dots_status("idle", "");
            }
            if let Some(hwnd) = win::hwnd_of(self.settings.window()) {
                win::focus_window(hwnd);
            }
            self.ensure_blur_tracking();
            // Next frame: animate in (fade + scale).
            let weak = self.weak.clone();
            Timer::single_shot(Duration::from_millis(16), move || {
                if let Some(app) = weak.upgrade() {
                    app.settings.set_shown(true);
                }
            });
        } else {
            self.settings.set_shown(false);
            let weak = self.weak.clone();
            let anim = (260.0 * self.anim_scale().max(0.1)) as u64;
            Timer::single_shot(Duration::from_millis(anim), move || {
                if let Some(app) = weak.upgrade() {
                    if !app.st.borrow().settings_visible {
                        let _ = app.settings.hide();
                    }
                }
            });
        }
    }

    pub(super) fn apply_settings_backdrop(&self) {
        if !self.st.borrow().settings_visible {
            return;
        }
        if let Some(hwnd) = win::hwnd_of(self.settings.window()) {
            let cfg = self.cfg.borrow();
            let _ = cfg;
            win::set_backdrop(hwnd, false, self.scheme.borrow().is_dark());
        }
    }

    pub(super) fn set_static_info(&self) {
        let about = system::about();
        let row = |l: &str, v: String| InfoRow { label: crate::i18n::tr(l).into(), value: v.into() };
        let system_rows = vec![
            row("Hostname", about.hostname.clone()),
            row("Device", about.device.clone()),
            row("OS", about.os.clone()),
            row("Build", about.build.clone()),
            row("Firmware", about.firmware.clone()),
        ];
        let software_rows = vec![
            row("Shell", env!("CARGO_PKG_VERSION").into()),
            row("UI toolkit", "Slint 1.18".into()),
            row("Renderer", "FemtoVG (OpenGL)".into()),
            row("Config format", "Caelestia shell.json".into()),
        ];
        self.settings.set_about_system(ModelRc::new(VecModel::from(system_rows)));
        self.settings.set_about_software(ModelRc::new(VecModel::from(software_rows)));
        self.settings.set_version(env!("CARGO_PKG_VERSION").into());

        // Caelestia reads the avatar from ~/.face.
        let face = dirs::home_dir().map(|h| h.join(".face"));
        let avatar = face.as_ref().and_then(|p| image::open(p).ok()).map(|img| {
            let img = img.thumbnail(128, 128).into_rgba8();
            Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(img.as_raw(), img.width(), img.height()))
        });
        let user = UserInfo {
            initial: about.user.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default().into(),
            name: about.user.into(),
            host: about.hostname.into(),
            uptime: crate::i18n::uptime(system::uptime_secs()).into(),
            os: about.os.into(),
            has_avatar: avatar.is_some(),
            avatar: avatar.unwrap_or_default(),
        };
        self.dashboard.global::<ShellGlobal>().set_user(user);
        each_window!(self, w => w.global::<ShellGlobal>().set_cookie_path(cookie_path().into()));
    }

    /// `dynamic` or `<name>:<flavour>` (e.g. `catppuccin:mocha`).
    pub(super) fn apply_scheme_id(&self, id: &str) {
        if id == "dynamic" {
            let (variant, dark) = {
                let s = self.scheme.borrow();
                (if s.is_dynamic() { s.variant.clone() } else { "tonalspot".into() }, s.is_dark())
            };
            // Mark as dynamic first so set_scheme regenerates from the wallpaper.
            self.scheme.borrow_mut().name = "dynamic".into();
            return self.set_scheme(Some(variant), Some(dark));
        }
        let (name, flavour) = id.split_once(':').unwrap_or((id, ""));
        if let Some(p) = crate::schemes::find(name, flavour) {
            let scheme = crate::schemes::to_scheme(p);
            if let Err(e) = scheme.save(&self.cfg.borrow().paths.scheme_json) {
                log::error!("saving scheme: {e:#}");
            }
            *self.scheme.borrow_mut() = scheme;
            self.apply_config();
        }
    }

    /// Opacity themes: one click sets enabled/base/layers together.
    pub(super) fn opacity_preset(&self, id: &str) {
        let (enabled, base, layers) = match id {
            "subtle" => (true, 0.92, 0.6),
            "translucent" => (true, 0.8, 0.45),
            "glass" => (true, 0.6, 0.3),
            "clear" => (true, 0.4, 0.2),
            _ => (false, 0.85, 0.4),
        };
        let result = {
            let mut cfg = self.cfg.borrow_mut();
            cfg.set("appearance.transparency.enabled", Value::Bool(enabled))
                .and_then(|_| cfg.set("appearance.transparency.base", Value::from(base)))
                .and_then(|_| cfg.set("appearance.transparency.layers", Value::from(layers)))
                .and_then(|_| cfg.save())
        };
        if let Err(e) = result {
            log::error!("saving opacity preset: {e:#}");
        }
        self.apply_config();
    }

    pub(super) fn refresh_scheme_cards(&self) {
        let current = {
            let s = self.scheme.borrow();
            if s.is_dynamic() { "dynamic".to_owned() } else { format!("{}:{}", s.name, s.flavour) }
        };
        let col = |v: u32| slint::Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8);
        let pal = self.scheme.borrow().palette();
        let mut cards = vec![SchemeCard {
            id: "dynamic".into(),
            label: crate::i18n::tr("Dynamic (wallpaper)").into(),
            dark: self.scheme.borrow().is_dark(),
            bg: if current == "dynamic" { pal.surface } else { col(0x1b1b21) },
            fg: if current == "dynamic" { pal.on_surface } else { col(0xe4e1e9) },
            c1: if current == "dynamic" { pal.primary } else { col(0xb5c4ff) },
            c2: if current == "dynamic" { pal.secondary } else { col(0xc1c5dd) },
            c3: if current == "dynamic" { pal.tertiary } else { col(0xe2bbdb) },
            current: current == "dynamic",
        }];
        cards.extend(crate::schemes::PRESETS.iter().map(|p| {
            let id = format!("{}:{}", p.name, p.flavour);
            SchemeCard {
                current: id == current,
                id: id.into(),
                label: p.label.into(),
                dark: p.dark,
                bg: col(p.base),
                fg: col(p.text),
                c1: col(p.primary),
                c2: col(p.secondary),
                c3: col(p.tertiary),
            }
        }));
        self.settings.set_schemes(ModelRc::new(VecModel::from(cards)));
    }

    pub(super) fn settings_search(&self, q: &str) {
        const KEYWORDS: [&str; 10] = [
            "wallpaper style fonts colours colors scheme transparency dark light theme rounding fondo estilo fuentes colores esquema transparencia oscuro claro tema opacidad",
            "network wifi wi-fi ethernet internet vpn red redes",
            "connected devices bluetooth pairing printers dispositivos emparejar impresoras auriculares",
            "audio volume sound microphone mute speakers volumen sonido microfono silenciar altavoces salida entrada",
            "updates system windows update winget actualizaciones sistema",
            "plugins extensions widgets extensiones complementos",
            "panels dashboard taskbar bar launcher sidebar border workspaces clock notifications clipboard performance battery paneles panel barra lanzador marco espacios reloj notificaciones portapapeles rendimiento bateria komorebi mosaico",
            "services poll intervals lyrics weather location servicios clima ubicacion ciudad",
            "language region locale weather location units fahrenheit clock 12-hour idioma region ubicacion unidades reloj espanol",
            "about system information credits version dots export import config github gallery acerca sistema informacion version exportar importar configuracion galeria",
        ];
        let q = q.trim().to_lowercase();
        let matches: Vec<bool> = if q.is_empty() { vec![] } else { KEYWORDS.iter().map(|k| q.split_whitespace().all(|w| k.contains(w))).collect() };
        self.settings.set_nav_match(ModelRc::new(VecModel::from(matches)));
    }

    pub(super) fn export_dots(&self) {
        let Some(dir) = rfd::FileDialog::new().set_title("Export dots to…").pick_folder() else { return };
        let out = dir.join("caelestia-dots");
        let result = (|| -> Result<()> {
            std::fs::create_dir_all(&out)?;
            let cfg = self.cfg.borrow();
            std::fs::write(out.join("shell.json"), serde_json::to_string_pretty(cfg.user_value())? + "\n")?;
            self.scheme.borrow().save(&out.join("scheme.json"))?;
            let fonts = cfg.paths.config_dir.join("fonts");
            if fonts.is_dir() {
                std::fs::create_dir_all(out.join("fonts"))?;
                for e in std::fs::read_dir(fonts)?.flatten() {
                    std::fs::copy(e.path(), out.join("fonts").join(e.file_name()))?;
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => system::reveal(&out.join("shell.json")),
            Err(e) => log::error!("export failed: {e:#}"),
        }
    }

    pub(super) fn import_dots(&self) {
        let Some(file) = rfd::FileDialog::new().set_title("Import shell.json or scheme.json").add_filter("Caelestia dots", &["json"]).pick_file() else { return };
        let result = (|| -> Result<()> {
            let text = std::fs::read_to_string(&file)?;
            let value: Value = serde_json::from_str(&text)?;
            if value.get("colours").is_some() {
                let scheme: Scheme = serde_json::from_value(value)?;
                scheme.save(&self.cfg.borrow().paths.scheme_json)?;
                *self.scheme.borrow_mut() = scheme;
            } else {
                self.cfg.borrow_mut().replace_with(value)?;
                // A shared dots folder may contain a matching scheme next to shell.json.
                let sibling = file.with_file_name("scheme.json");
                if let Ok(s) = Scheme::load(&sibling) {
                    s.save(&self.cfg.borrow().paths.scheme_json)?;
                    *self.scheme.borrow_mut() = s;
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => self.apply_config(),
            Err(e) => log::error!("import failed: {e:#}"),
        }
    }
}
