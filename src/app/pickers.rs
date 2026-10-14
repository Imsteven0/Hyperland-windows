//! Live-preview pickers in the launcher (scheme, variant, opacity).

use super::*;

impl Caelestia {
    /// Seed colour of the current wallpaper (cached; decoding is ~100 ms).
    pub(super) fn wallpaper_source(&self) -> material_colors::color::Rgb {
        let wp = self.st.borrow().wallpaper.clone();
        if let (Some((p, rgb)), Some(w)) = (&self.st.borrow().source_rgb, &wp) {
            if p == w {
                return *rgb;
            }
        }
        let rgb = wp.as_ref().and_then(|p| source_from_image(p).ok()).unwrap_or(material_colors::color::Rgb::from_u32(0x4a6fa5));
        if let Some(w) = wp {
            self.st.borrow_mut().source_rgb = Some((w, rgb));
        }
        rgb
    }

    pub(super) fn open_picker(&self, kind: &'static str) {
        // Switching pickers: undo the previous preview before snapshotting state.
        self.picker_cancel();
        self.launcher.set_picker_mode("".into());
        let tr = crate::i18n::tr;
        let col = |c: slint::Color| c;
        let rgb = |v: u32| slint::Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8);
        let (scheme, dark) = {
            let s = self.scheme.borrow();
            (s.clone(), s.is_dark())
        };
        let mut ids = Vec::new();
        let mut entries = Vec::new();
        let mut current = 0usize;
        let entry = |id: &str, label: String, sub: String, p: &crate::Palette, cur: bool| PickerEntry {
            id: id.into(),
            label: label.into(),
            sublabel: sub.into(),
            bg: col(p.surface),
            fg: col(p.on_surface),
            c1: col(p.primary),
            c2: col(p.secondary),
            c3: col(p.tertiary),
            alpha: 1.0,
            current: cur,
        };
        let title = match kind {
            "variant" => {
                let source = self.wallpaper_source();
                const ORDER: [&str; 9] = ["tonalspot", "vibrant", "expressive", "fidelity", "content", "fruitsalad", "rainbow", "neutral", "monochrome"];
                for v in ORDER {
                    let p = Scheme::generate(source, v, dark).palette();
                    let cur = scheme.is_dynamic() && scheme.variant == v;
                    if cur {
                        current = ids.len();
                    }
                    entries.push(entry(v, v.to_owned(), if cur { tr("Current") } else { String::new() }, &p, cur));
                    ids.push(v.to_owned());
                }
                tr("Dynamic variant")
            }
            "scheme" => {
                let source = self.wallpaper_source();
                let dynamic = Scheme::generate(source, if scheme.is_dynamic() { &scheme.variant } else { "tonalspot" }, dark).palette();
                let cur_id = if scheme.is_dynamic() { "dynamic".to_owned() } else { format!("{}:{}", scheme.name, scheme.flavour) };
                entries.push(entry("dynamic", tr("Dynamic (wallpaper)"), String::new(), &dynamic, cur_id == "dynamic"));
                ids.push("dynamic".into());
                for p in crate::schemes::PRESETS {
                    let id = format!("{}:{}", p.name, p.flavour);
                    if id == cur_id {
                        current = ids.len();
                    }
                    let mut e = entry(&id, p.label.to_owned(), tr(if p.dark { "Dark" } else { "Light" }), &crate::schemes::to_scheme(p).palette(), id == cur_id);
                    e.bg = rgb(p.base);
                    e.fg = rgb(p.text);
                    entries.push(e);
                    ids.push(id);
                }
                tr("Colour scheme")
            }
            _ => {
                let cfg = self.cfg.borrow();
                let (on, base) = (cfg.bool("appearance.transparency.enabled"), cfg.f64("appearance.transparency.base"));
                for (id, label, alpha) in [("solid", "Solid", 1.0f32), ("subtle", "Subtle", 0.92), ("translucent", "Translucent", 0.8), ("glass", "Glass", 0.6), ("clear", "Clear", 0.4)] {
                    let cur = if id == "solid" { !on } else { on && (base as f32 - alpha).abs() < 0.02 };
                    if cur {
                        current = ids.len();
                    }
                    let mut e = entry(id, tr(label), format!("{}%", (alpha * 100.0).round()), &scheme.palette(), cur);
                    e.alpha = alpha;
                    entries.push(e);
                    ids.push(id.to_owned());
                }
                tr("Opacity")
            }
        };
        let original_opacity = {
            let cfg = self.cfg.borrow();
            (
                cfg.get("appearance.transparency.enabled").cloned().unwrap_or(Value::Bool(false)),
                cfg.get("appearance.transparency.base").cloned().unwrap_or(Value::from(0.85)),
                cfg.get("appearance.transparency.layers").cloned().unwrap_or(Value::from(0.4)),
            )
        };
        if !self.st.borrow().drawers[Drawer::Launcher as usize].open {
            self.open(Drawer::Launcher, false);
        }
        self.st.borrow_mut().picker = Some(Picker { kind, ids, original_scheme: scheme, original_opacity });
        // Selection first (no preview yet), then switch the launcher into picker mode.
        self.launcher.set_picker_entries(ModelRc::new(VecModel::from(entries)));
        self.launcher.set_picker_title(title.into());
        self.launcher.set_selected(current as i32);
        self.launcher.set_picker_mode(kind.into());
        self.focus_drawer(Drawer::Launcher);
    }

    /// Applies option `i` to the live UI without saving anything.
    pub(super) fn picker_preview(&self, i: usize) {
        let Some((kind, id)) = self.st.borrow().picker.as_ref().and_then(|p| p.ids.get(i).map(|id| (p.kind, id.clone()))) else { return };
        match kind {
            "variant" => {
                let dark = self.scheme.borrow().is_dark();
                *self.scheme.borrow_mut() = Scheme::generate(self.wallpaper_source(), &id, dark);
            }
            "scheme" => {
                let dark = self.scheme.borrow().is_dark();
                let next = if id == "dynamic" {
                    let variant = self.st.borrow().picker.as_ref().map(|p| p.original_scheme.clone()).filter(|s| s.is_dynamic()).map(|s| s.variant).unwrap_or_else(|| "tonalspot".into());
                    Scheme::generate(self.wallpaper_source(), &variant, dark)
                } else {
                    let (name, flavour) = id.split_once(':').unwrap_or((&id, ""));
                    match crate::schemes::find(name, flavour) {
                        Some(p) => crate::schemes::to_scheme(p),
                        None => return,
                    }
                };
                *self.scheme.borrow_mut() = next;
            }
            _ => {
                let (enabled, base, layers) = match id.as_str() {
                    "subtle" => (true, 0.92, 0.6),
                    "translucent" => (true, 0.8, 0.45),
                    "glass" => (true, 0.6, 0.3),
                    "clear" => (true, 0.4, 0.2),
                    _ => (false, 0.85, 0.4),
                };
                let mut cfg = self.cfg.borrow_mut();
                let _ = cfg.set("appearance.transparency.enabled", Value::Bool(enabled));
                let _ = cfg.set("appearance.transparency.base", Value::from(base));
                let _ = cfg.set("appearance.transparency.layers", Value::from(layers));
            }
        }
        self.apply_config();
    }

    /// Keeps option `i`: persists it and closes the launcher.
    pub(super) fn picker_confirm(&self, i: usize) {
        self.picker_preview(i);
        let Some(picker) = self.st.borrow_mut().picker.take() else { return };
        let result = if picker.kind == "opacity" { self.cfg.borrow_mut().save() } else { self.scheme.borrow().save(&self.cfg.borrow().paths.scheme_json) };
        if let Err(e) = result {
            log::error!("saving picker choice: {e:#}");
        }
        self.st.borrow_mut().komorebi_visuals = None;
        self.apply_config(); // also syncs komorebi borders now that the picker is done
        self.close(Drawer::Launcher);
    }

    /// Esc / closing the launcher: restore what was there before previewing.
    pub(super) fn picker_cancel(&self) {
        let Some(picker) = self.st.borrow_mut().picker.take() else { return };
        *self.scheme.borrow_mut() = picker.original_scheme;
        {
            let mut cfg = self.cfg.borrow_mut();
            let (e, b, l) = picker.original_opacity;
            let _ = cfg.set("appearance.transparency.enabled", e);
            let _ = cfg.set("appearance.transparency.base", b);
            let _ = cfg.set("appearance.transparency.layers", l);
        }
        self.apply_config();
    }
}
