//! Launcher: apps, `>` actions, calculator, emoji and clipboard history.

use super::*;

impl Caelestia {
    pub(super) fn reindex_apps(&self) {
        let hidden = self.cfg.borrow().str_list("launcher.hiddenApps");
        let px = (32.0 * self.primary_scale()).round() as i32;
        spawn_index(hidden, px.max(32), |records| post(move |app| app.on_apps(records)));
    }

    pub(super) fn on_apps(&self, records: Vec<AppRecord>) {
        {
            let mut st = self.st.borrow_mut();
            st.apps = records
                .into_iter()
                .map(|r| AppItem { name: r.name, description: r.description, launch: r.launch, icon: r.icon.map(Image::from_rgba8_premultiplied) })
                .collect();
            st.apps_indexed_at = Some(Instant::now());
        }
        if self.st.borrow().drawers[Drawer::Launcher as usize].open {
            let q = self.launcher.get_query();
            self.search(&q);
        }
    }

    pub(super) fn launcher_prefixes(&self) -> Prefixes {
        let cfg = self.cfg.borrow();
        Prefixes { actions: cfg.str("launcher.actionPrefix"), calc: cfg.str("launcher.calcPrefix"), emoji: cfg.str("launcher.emojiPrefix"), clipboard: cfg.str("launcher.clipboardPrefix") }
    }

    pub(super) fn search(&self, query: &str) {
        let tr = crate::i18n::tr;
        let es = crate::i18n::is_es();
        let (mode, rest) = self.launcher_prefixes().split(query);
        let (dangerous, clip_on) = {
            let cfg = self.cfg.borrow();
            (cfg.bool("launcher.enableDangerousActions"), cfg.bool("launcher.clipboardHistory"))
        };
        let row = |name: String, description: String, glyph: &str, emoji: &str| AppEntry {
            id: 0,
            name: name.into(),
            description: description.into(),
            icon: Image::default(),
            has_icon: false,
            is_action: true,
            glyph: glyph.into(),
            emoji: emoji.into(),
        };
        let mut st = self.st.borrow_mut();
        let st = &mut *st;
        let mut rows: Vec<(Hit, AppEntry)> = Vec::new();
        match mode {
            LauncherMode::Actions => {
                let actions: Vec<&Action> = ACTIONS.iter().filter(|a| dangerous || !a.dangerous).collect();
                let q = rest.trim();
                let picked: Vec<usize> = if q.is_empty() {
                    (0..actions.len()).collect()
                } else {
                    let names: Vec<String> = actions.iter().map(|a| tr(a.name)).collect();
                    st.fuzzy.rank(q, names.iter().map(String::as_str), 50)
                };
                rows.extend(picked.into_iter().map(|i| {
                    let a = *actions[i];
                    (Hit::Action(a), row(tr(a.name), tr(a.description), a.icon, ""))
                }));
            }
            LauncherMode::Apps => {
                let picked: Vec<usize> = if query.trim().is_empty() {
                    (0..st.apps.len().min(60)).collect()
                } else {
                    let names: Vec<&str> = st.apps.iter().map(|a| a.name.as_str()).collect();
                    st.fuzzy.rank(query.trim(), names.into_iter(), 50)
                };
                rows.extend(picked.into_iter().map(|i| {
                    let a = &st.apps[i];
                    let entry = AppEntry {
                        id: 0,
                        name: a.name.clone().into(),
                        description: a.description.clone().into(),
                        has_icon: a.icon.is_some(),
                        icon: a.icon.clone().unwrap_or_default(),
                        is_action: false,
                        glyph: SharedString::new(),
                        emoji: SharedString::new(),
                    };
                    (Hit::App(i), entry)
                }));
            }
            LauncherMode::Calc => {
                let expr = rest.trim();
                rows.push(match calc::eval(expr) {
                    Ok(v) => {
                        let result = calc::format(v, es);
                        (Hit::Copy(result.clone()), row(result, format!("{expr} = · {}", tr("Enter copies the result")), "calculate", ""))
                    }
                    Err(CalcError::Empty) => (Hit::Hint, row(tr("Calculator"), tr("Type an expression, e.g. 2+2, sqrt(16)*pi, 90deg"), "calculate", "")),
                    Err(e) => {
                        let why = match e {
                            CalcError::Undefined => tr("Undefined result"),
                            CalcError::UnknownName(n) => format!("{}: {n}", tr("Unknown name")),
                            _ => tr("Keep typing…"),
                        };
                        (Hit::Hint, row(expr.to_owned(), why, "calculate", ""))
                    }
                });
            }
            LauncherMode::Emoji => {
                rows.extend(emoji::search(rest, 50).into_iter().map(|e| {
                    let codes = e.shortcodes.iter().map(|c| format!(":{c}:")).collect::<Vec<_>>().join(" ");
                    (Hit::Copy(e.emoji.to_owned()), row(e.label(es), codes, "", e.emoji))
                }));
                if rows.is_empty() {
                    rows.push((Hit::Hint, row(tr("No emoji found"), tr("Try another word, in English or Spanish"), "mood", "")));
                }
            }
            LauncherMode::Clipboard => {
                let q = rest.trim();
                let picked: Vec<usize> = if q.is_empty() {
                    (0..st.clips.len()).collect()
                } else {
                    // Rank on the start of each entry: enough to find it, cheap on big copies.
                    let heads: Vec<String> = st.clips.iter().map(|c| c.text.chars().take(300).collect()).collect();
                    st.fuzzy.rank(q, heads.iter().map(String::as_str), 50)
                };
                let now = std::time::SystemTime::now();
                rows.extend(picked.into_iter().filter_map(|i| {
                    let item = st.clips.get(i)?;
                    let age = crate::i18n::ago(now.duration_since(item.at).map(|d| d.as_secs()).unwrap_or(0));
                    let size = match (item.lines(), es) {
                        (1, true) => format!("{} caracteres", item.text.chars().count()),
                        (1, false) => format!("{} characters", item.text.chars().count()),
                        (n, true) => format!("{n} líneas"),
                        (n, false) => format!("{n} lines"),
                    };
                    Some((Hit::Clip(i), row(item.preview(90), format!("{age} · {size}"), "content_paste", "")))
                }));
                if rows.is_empty() {
                    let (title, sub) = if !clip_on {
                        (tr("Clipboard history is off"), tr("Turn it on in Settings › Panels"))
                    } else if st.clips.is_empty() {
                        (tr("Nothing copied yet"), tr("Text you copy appears here · Shift+Del removes an entry"))
                    } else {
                        (tr("No matches"), String::new())
                    };
                    rows.push((Hit::Hint, row(title, sub, "content_paste", "")));
                }
            }
        }
        let (hits, mut entries): (Vec<Hit>, Vec<AppEntry>) = rows.into_iter().unzip();
        for (i, e) in entries.iter_mut().enumerate() {
            e.id = i as i32;
        }
        st.hits = hits;
        self.launcher.set_results(ModelRc::new(VecModel::from(entries)));
    }

    /// Opens the launcher already in a prefix mode (hotkey / IPC / `>` action).
    pub(super) fn open_launcher_mode(&self, mode: LauncherMode) {
        if !self.st.borrow().drawers[Drawer::Launcher as usize].open {
            self.open(Drawer::Launcher, false);
        }
        let prefix = self.launcher_prefixes().get(mode).to_owned();
        self.launcher.set_query(prefix.clone().into());
        self.search(&prefix);
        self.focus_drawer(Drawer::Launcher);
    }

    pub(super) fn activate_hit(&self, index: usize) {
        let target = {
            let st = self.st.borrow();
            match st.hits.get(index) {
                Some(Hit::App(i)) => st.apps.get(*i).map(|a| LauncherTarget::Launch(a.launch.clone())),
                Some(Hit::Action(a)) => Some(LauncherTarget::Action(*a)),
                Some(Hit::Copy(text)) => Some(LauncherTarget::Copy(text.clone())),
                Some(Hit::Clip(i)) => st.clips.get(*i).map(|c| LauncherTarget::Copy(c.text.clone())),
                Some(Hit::Hint) | None => None,
            }
        };
        match target {
            Some(LauncherTarget::Launch(launch)) => {
                self.close(Drawer::Launcher);
                system::open(&format!("shell:AppsFolder\\{launch}"));
            }
            Some(LauncherTarget::Action(action)) => self.run_launcher_action(action),
            Some(LauncherTarget::Copy(text)) => self.copy_and_close(&text),
            None => {}
        }
    }

    /// Puts `text` on the clipboard, closes the launcher and, if configured
    /// (`launcher.clipboardPaste`), pastes it into the window behind.
    pub(super) fn copy_and_close(&self, text: &str) {
        if !crate::platform::clipboard::set_text(text) {
            log::warn!("could not write to the clipboard");
            return;
        }
        self.close(Drawer::Launcher);
        if self.cfg.borrow().bool("launcher.clipboardPaste") {
            Timer::single_shot(Duration::from_millis(180), || {
                // Only into another app, never into our own surfaces.
                if desktops::window_info(win::foreground_window()).is_some() {
                    win::send_keys(&[0x11], &[0x56]); // Ctrl+V
                }
            });
        }
    }

    /// Shift+Delete in the launcher: forget a clipboard history entry.
    pub(super) fn remove_launcher_entry(&self, index: usize) {
        let removed = {
            let mut st = self.st.borrow_mut();
            match st.hits.get(index) {
                Some(Hit::Clip(i)) => {
                    let i = *i;
                    st.clips.remove(i).is_some()
                }
                _ => false,
            }
        };
        if removed {
            let q = self.launcher.get_query();
            self.search(&q);
            self.launcher.set_selected(index.saturating_sub(1) as i32);
        }
    }

    /// Starts/stops the clipboard watcher to match `launcher.clipboardHistory`.
    pub(super) fn sync_clipboard_watcher(&self) {
        let (on, max) = {
            let cfg = self.cfg.borrow();
            (cfg.bool("launcher.clipboardHistory"), cfg.i64("launcher.clipboardMaxItems").clamp(5, 500) as usize)
        };
        self.st.borrow_mut().clips.set_max(max);
        let mut watcher = self.clip_watcher.borrow_mut();
        if on && watcher.is_none() {
            *watcher = crate::platform::clipboard::watch(|text| post(move |app| app.on_clipboard_text(text)));
            if watcher.is_none() {
                log::warn!("clipboard history unavailable");
            }
        } else if !on && watcher.is_some() {
            *watcher = None;
            self.st.borrow_mut().clips.clear();
        }
    }

    pub(super) fn on_clipboard_text(&self, text: String) {
        if !self.st.borrow_mut().clips.push(text, std::time::SystemTime::now()) {
            return;
        }
        if self.st.borrow().drawers[Drawer::Launcher as usize].open {
            let q = self.launcher.get_query();
            if self.launcher_prefixes().split(&q).0 == LauncherMode::Clipboard {
                self.search(&q);
            }
        }
    }

    pub(super) fn run_launcher_action(&self, a: Action) {
        match a.id {
            "wallpaper" => return self.open_wallpaper_picker(),
            "random-wallpaper" => self.random_wallpaper(),
            "light" => self.set_scheme(None, Some(false)),
            "dark" => self.set_scheme(None, Some(true)),
            "variant" | "transparency" | "scheme" => {
                return self.open_picker(match a.id {
                    "variant" => "variant",
                    "transparency" => "opacity",
                    _ => "scheme",
                });
            }
            "settings" => {
                self.close(Drawer::Launcher);
                return self.toggle_settings(Some(true));
            }
            "dashboard" => return self.open(Drawer::Dashboard, false),
            "calc" => return self.open_launcher_mode(LauncherMode::Calc),
            "emoji" => return self.open_launcher_mode(LauncherMode::Emoji),
            "clipboard" => return self.open_launcher_mode(LauncherMode::Clipboard),
            "reload" => self.action("reload-config", ""),
            "confetti" => self.play_confetti(),
            "lock" | "sleep" | "logout" | "reboot" | "shutdown" => {
                self.close(Drawer::Launcher);
                return system::session(a.id);
            }
            "quit" => {
                let _ = slint::quit_event_loop();
            }
            _ => {}
        }
        self.close(Drawer::Launcher);
    }
}
