//! Dots gallery: importing a configuration from a GitHub link, with backup and undo.

use super::*;

impl Caelestia {
    pub(super) fn dots_backup_dir(&self) -> PathBuf {
        self.cfg.borrow().paths.state_dir.join("dots-backup")
    }

    pub(super) fn dots_recent_path(&self) -> PathBuf {
        self.cfg.borrow().paths.state_dir.join("dots-recent.json")
    }

    pub(super) fn set_dots_status(&self, state: &str, text: &str) {
        let g = self.settings.global::<ShellGlobal>();
        g.set_dots_state(state.into());
        g.set_dots_text(text.into());
        g.set_dots_can_undo(self.dots_backup_dir().join("shell.json").is_file());
        let recent: Vec<SharedString> = std::fs::read_to_string(self.dots_recent_path())
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect();
        g.set_dots_recent(ModelRc::new(VecModel::from(recent)));
    }

    /// Step 1: index the repository and download its dots for a preview.
    pub(super) fn fetch_dots(&self, url: &str) {
        let tr = crate::i18n::tr;
        let generation = {
            let mut st = self.st.borrow_mut();
            st.dots_gen += 1;
            st.dots_preview = None;
            st.dots_gen
        };
        self.set_dots_status("fetching", &tr("Reading the repository…"));
        let url = url.trim().to_owned();
        std::thread::spawn(move || {
            let result = dots_repo::fetch(&url);
            post(move |app| app.on_dots_preview(generation, &url, result));
        });
    }

    pub(super) fn on_dots_preview(&self, generation: u32, url: &str, result: Result<dots_repo::Preview>) {
        let tr = crate::i18n::tr;
        if self.st.borrow().dots_gen != generation {
            return;
        }
        match result {
            Ok(p) => {
                let mut parts = Vec::new();
                if p.shell.is_some() {
                    parts.push("shell.json".to_owned());
                }
                if let Some(s) = &p.scheme {
                    parts.push(format!("scheme.json ({})", if s.is_dynamic() { tr("dynamic") } else { s.name.clone() }));
                }
                let walls = p.layout.wallpapers.as_ref().map(|w| w.1).unwrap_or(0);
                if walls > 0 {
                    parts.push(if crate::i18n::is_es() { format!("{walls} fondos") } else { format!("{walls} wallpapers") });
                }
                let text = format!("{}/{} · {}", p.repo.owner, p.repo.name, parts.join(" · "));
                self.settings.global::<ShellGlobal>().set_dots_wallpapers(walls as i32);
                self.st.borrow_mut().dots_preview = Some(p);
                self.remember_dots_source(url);
                self.set_dots_status("ready", &text);
            }
            Err(e) => {
                log::warn!("dots import: {e:#}");
                // Messages are short English sentences the i18n table knows.
                self.set_dots_status("error", &format!("{}: {}", tr("Error"), tr(&e.to_string())));
            }
        }
    }

    pub(super) fn remember_dots_source(&self, url: &str) {
        let path = self.dots_recent_path();
        let mut list: Vec<String> = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        list.retain(|u| u != url);
        list.insert(0, url.to_owned());
        list.truncate(8);
        if let Ok(text) = serde_json::to_string_pretty(&list) {
            let _ = crate::config::write_atomic(&path, &text);
        }
    }

    /// Step 2: back up the current dots, then apply the preview (keeping this
    /// machine's personal settings) and optionally fetch its wallpapers.
    pub(super) fn apply_dots(&self, with_wallpapers: bool) {
        let tr = crate::i18n::tr;
        let Some(p) = self.st.borrow_mut().dots_preview.take() else { return };
        let backup = self.dots_backup_dir();
        let result = (|| -> Result<()> {
            std::fs::create_dir_all(&backup)?;
            {
                let cfg = self.cfg.borrow();
                crate::config::write_atomic(&backup.join("shell.json"), &(serde_json::to_string_pretty(cfg.user_value())? + "\n"))?;
            }
            self.scheme.borrow().save(&backup.join("scheme.json"))?;
            if let Some(shell) = &p.shell {
                let merged = dots_repo::merge_imported(self.cfg.borrow().user_value(), shell);
                self.cfg.borrow_mut().replace_with(merged)?;
            }
            if let Some(scheme) = &p.scheme {
                scheme.save(&self.cfg.borrow().paths.scheme_json)?;
                *self.scheme.borrow_mut() = scheme.clone();
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                log::info!("applied dots from {}/{}", p.repo.owner, p.repo.name);
                self.st.borrow_mut().komorebi_visuals = None;
                self.apply_config();
                self.set_dots_status("applied", &format!("{} · {}/{}", tr("Applied"), p.repo.owner, p.repo.name));
                if with_wallpapers {
                    if let Some(url) = p.wallpapers_url() {
                        self.start_wallpaper_download(&url);
                    }
                }
            }
            Err(e) => {
                log::error!("applying dots: {e:#}");
                self.set_dots_status("error", &format!("{}: {e}", tr("Error")));
            }
        }
    }

    /// Restores the dots saved by the last import.
    pub(super) fn undo_dots(&self) {
        let tr = crate::i18n::tr;
        let backup = self.dots_backup_dir();
        let result = (|| -> Result<()> {
            let user: Value = serde_json::from_str(&std::fs::read_to_string(backup.join("shell.json"))?)?;
            self.cfg.borrow_mut().replace_with(user)?;
            if let Ok(s) = Scheme::load(&backup.join("scheme.json")) {
                s.save(&self.cfg.borrow().paths.scheme_json)?;
                *self.scheme.borrow_mut() = s;
            }
            std::fs::remove_dir_all(&backup)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.st.borrow_mut().komorebi_visuals = None;
                self.apply_config();
                self.set_dots_status("idle", &tr("Previous dots restored"));
            }
            Err(e) => {
                log::error!("restoring dots: {e:#}");
                self.set_dots_status("error", &format!("{}: {e}", tr("Error")));
            }
        }
    }
}
