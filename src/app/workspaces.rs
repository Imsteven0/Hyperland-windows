//! Workspaces in the bar: komorebi (tiling) or Windows virtual desktops, and komorebi theming.

use super::*;

impl Caelestia {
    pub(super) fn komorebi_active(&self) -> bool {
        self.cfg.borrow().bool("windows.komorebi.enabled") && matches!(self.st.borrow().komorebi, KStatus::Running(_))
    }

    pub(super) fn on_komorebi(&self, status: KStatus) {
        // Workspace switches cloak apps: fix their overlays right away.
        self.ghosts.borrow_mut().scan();
        let was_running = matches!(self.st.borrow().komorebi, KStatus::Running(_));
        let label = match &status {
            KStatus::NotInstalled => "not-installed",
            KStatus::Stopped => "stopped",
            KStatus::Running(s) if s.paused => "paused",
            KStatus::Running(_) => "running",
        };
        self.settings.global::<ShellGlobal>().set_komorebi_status(label.into());
        let enabled = self.cfg.borrow().bool("windows.komorebi.enabled");
        if matches!(status, KStatus::Stopped) && enabled && self.cfg.borrow().bool("windows.komorebi.autostart") && !self.st.borrow().komorebi_autostarted {
            self.st.borrow_mut().komorebi_autostarted = true;
            log::info!("starting komorebi (without its bar and hotkey daemon)");
            self.push_komorebi_theme();
            komorebi::start();
        }
        let running = matches!(status, KStatus::Running(_));
        let state = match &status {
            KStatus::Running(s) => Some(s.clone()),
            _ => None,
        };
        self.st.borrow_mut().komorebi = status;
        if running != was_running {
            // Switch the bar between komorebi and virtual-desktop workspaces.
            self.st.borrow_mut().desktops = Desktops::default();
            self.st.borrow_mut().komorebi_colours.clear();
            self.bind_hotkeys();
            if !running {
                self.refresh_workspaces();
            } else {
                // Our AppBars may predate komorebi: make it re-read work areas.
                komorebi::command(&["retile"]);
            }
        }
        if let (Some(state), true) = (state, enabled) {
            self.apply_komorebi_workspaces(&state);
            self.push_komorebi_theme();
        }
    }

    pub(super) fn komorebi_monitor_for(&self, screen: usize, state: &KState) -> Option<usize> {
        let screens = self.screens.borrow();
        let s = screens.get(screen)?;
        state
            .monitors
            .iter()
            .position(|m| m.left == s.monitor.rect.left && m.top == s.monitor.rect.top)
            .or_else(|| (screen < state.monitors.len()).then_some(screen))
    }

    pub(super) fn apply_komorebi_workspaces(&self, state: &KState) {
        for (i, s) in self.screens.borrow().iter().enumerate() {
            let Some(km) = self.komorebi_monitor_for_rect(&s.monitor, state, i) else { continue };
            let m = &state.monitors[km];
            let entries: Vec<(String, Vec<isize>)> = m.workspaces.iter().map(|w| (w.name.clone(), w.windows.clone())).collect();
            let g = s.bar.global::<ShellGlobal>();
            g.set_workspaces(self.workspace_model(&entries));
            set_active_workspace(&g, m.focused as i32 + 1);
        }
    }

    /// Builds the bar model: one entry per workspace with up to 5 unique app
    /// icons and the running totals the bar uses to lay out slots.
    pub(super) fn workspace_model(&self, entries: &[(String, Vec<isize>)]) -> ModelRc<Workspace> {
        const MAX_ICONS: usize = 5;
        let (mut icons_before, mut occupied_before) = (0i32, 0i32);
        let model: Vec<Workspace> = entries
            .iter()
            .enumerate()
            .map(|(i, (name, windows))| {
                let mut seen = std::collections::HashSet::new();
                let imgs: Vec<Image> = windows
                    .iter()
                    .filter_map(|&h| icons::window_key(h))
                    .filter(|k| seen.insert(k.clone())) // one icon per app
                    .filter_map(|k| self.icons.get(&k))
                    .take(MAX_ICONS)
                    .collect();
                let count = imgs.len() as i32;
                let ws = Workspace {
                    id: i as i32 + 1,
                    occupied: !windows.is_empty(),
                    name: name.clone().into(),
                    icons: ModelRc::new(VecModel::from(imgs)),
                    icon_count: count,
                    icons_before,
                    occupied_before,
                };
                icons_before += count;
                occupied_before += (count > 0) as i32;
                ws
            })
            .collect();
        ModelRc::new(VecModel::from(model))
    }

    /// An icon finished extracting: rebuild the workspace models.
    pub(super) fn refresh_workspace_icons(&self) {
        if let KStatus::Running(state) = self.st.borrow().komorebi.clone() {
            if self.komorebi_active() {
                return self.apply_komorebi_workspaces(&state);
            }
        }
        self.st.borrow_mut().desktops = Desktops::default();
        self.refresh_workspaces();
    }

    pub(super) fn komorebi_monitor_for_rect(&self, mon: &Monitor, state: &KState, fallback: usize) -> Option<usize> {
        state
            .monitors
            .iter()
            .position(|m| m.left == mon.rect.left && m.top == mon.rect.top)
            .or_else(|| (fallback < state.monitors.len()).then_some(fallback))
    }

    /// Workspace click/scroll on a given screen's bar.
    pub(super) fn komorebi_workspace(&self, screen: usize, id: &str, arg: &str) {
        let KStatus::Running(state) = self.st.borrow().komorebi.clone() else { return };
        let Some(km) = self.komorebi_monitor_for(screen, &state) else { return };
        let m = &state.monitors[km];
        let count = m.workspaces.len().max(1) as i64;
        let target = if id == "workspace" {
            arg.parse::<i64>().unwrap_or(1) - 1
        } else {
            (m.focused as i64 + arg.parse::<i64>().unwrap_or(1)).rem_euclid(count)
        };
        if target >= 0 && target < count {
            komorebi::command(&["focus-monitor-workspace", &km.to_string(), &target.to_string()]);
        }
    }

    /// Writes the dots-driven visual part of komorebi.json (gaps, borders in
    /// the scheme's colours, animations) and reloads komorebi if it changed.
    pub(super) fn push_komorebi_theme(&self) {
        let cfg = self.cfg.borrow();
        if !cfg.bool("windows.komorebi.enabled") {
            return;
        }
        let p = self.scheme.borrow().palette();
        let hex = |c: slint::Color| format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue());
        let visuals = komorebi::Visuals {
            workspace_padding: cfg.i64("windows.komorebi.workspacePadding"),
            container_padding: cfg.i64("windows.komorebi.containerPadding"),
            borders: cfg.bool("windows.komorebi.borders"),
            border_width: cfg.i64("windows.komorebi.borderWidth"),
            border_offset: cfg.i64("windows.komorebi.borderOffset"),
            colours: vec![
                ("single", hex(p.primary)),
                ("stack", hex(p.secondary)),
                ("monocle", hex(p.tertiary)),
                ("floating", hex(p.tertiary)),
                ("unfocused", hex(p.outline_variant)),
            ],
            animations: cfg.bool("windows.komorebi.animations"),
            animation_ms: (250.0 * cfg.f64("appearance.anim.durations.scale").max(0.1)).round() as i64,
            workspaces: cfg.i64("bar.workspaces.shown").clamp(1, 10) as usize,
            // Real monitor count (screens may not be enumerated yet at startup).
            monitors: win::monitors().len(),
            layered_apps: cfg.str_list("windows.komorebi.layeredApps"),
        };
        drop(cfg);
        let key = format!("{visuals:?}");
        if self.st.borrow().komorebi_colours == key {
            return;
        }
        match komorebi::sync_config(&visuals) {
            Ok(synced) => {
                self.st.borrow_mut().komorebi_colours = key;
                if matches!(self.st.borrow().komorebi, KStatus::Running(_)) {
                    if synced.structure {
                        log::info!("komorebi.json updated from the dots, reloading komorebi");
                        komorebi::reload_config();
                    } else if synced.colours {
                        // Wallpaper/scheme changes: recolour live, keep the layout.
                        komorebi::apply_colours(&visuals.colours);
                    }
                }
            }
            Err(e) => log::warn!("could not update komorebi.json: {e:#}"),
        }
    }

    pub(super) fn refresh_workspaces(&self) {
        if self.komorebi_active() {
            return;
        }
        let d = desktops::query();
        if self.st.borrow().desktops == d {
            return;
        }
        let entries: Vec<(String, Vec<isize>)> = d.windows.iter().enumerate().map(|(i, w)| ((i + 1).to_string(), w.clone())).collect();
        let model = self.workspace_model(&entries);
        for s in self.screens.borrow().iter() {
            let g = s.bar.global::<ShellGlobal>();
            g.set_workspaces(model.clone());
            set_active_workspace(&g, d.current as i32);
        }
        self.st.borrow_mut().desktops = d;
    }
}
