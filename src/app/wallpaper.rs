//! Wallpapers: applying (with live preview), the launcher carousel, the Settings grid and GitHub downloads.

use super::*;

impl Caelestia {
    pub(super) fn set_wallpaper(&self, path: PathBuf) {
        self.apply_wallpaper(path, true);
    }

    /// Shows `path` on the desktop and recolours the shell from it. `persist`
    /// false is the launcher's live preview: nothing is written to disk.
    pub(super) fn apply_wallpaper(&self, path: PathBuf, persist: bool) {
        let (manage, smart, paths) = {
            let cfg = self.cfg.borrow();
            (cfg.bool("background.enabled"), cfg.bool("services.smartScheme"), cfg.paths.clone())
        };
        let generation = {
            let mut st = self.st.borrow_mut();
            st.wallpaper_gen += 1;
            st.wallpaper_gen
        };
        if manage {
            system::set_wallpaper_async(path.clone());
        }
        if persist {
            let _ = std::fs::create_dir_all(paths.state_dir.join("wallpaper"));
            let _ = std::fs::write(paths.state_dir.join("wallpaper").join("path.txt"), path.display().to_string());
        }
        self.st.borrow_mut().wallpaper = Some(path.clone());
        if persist {
            self.update_wallpaper_preview();
        }
        self.mark_current_wallpaper();
        let (variant, dark, dynamic) = {
            let s = self.scheme.borrow();
            (s.variant.clone(), s.is_dark(), s.is_dynamic())
        };
        if dynamic {
            std::thread::spawn(move || {
                let Ok(source) = source_from_image(&path) else { return };
                // smartScheme: pick light/dark from the wallpaper's luminance, like Caelestia.
                let dark = if smart { average_luminance(&path).map(|l| l < 0.6).unwrap_or(dark) } else { dark };
                let scheme = Scheme::generate(source, &variant, dark);
                if persist {
                    let _ = scheme.save(&paths.scheme_json);
                }
                post(move |app| {
                    if app.st.borrow().wallpaper_gen != generation {
                        return; // another wallpaper was picked meanwhile
                    }
                    *app.scheme.borrow_mut() = scheme;
                    app.apply_config();
                });
            });
        }
    }

    /// Launcher browsing: applies the selected wallpaper once the selection
    /// rests for a moment (holding an arrow key doesn't thrash the desktop).
    pub(super) fn preview_wallpaper(&self, i: usize) {
        let path = {
            let st = self.st.borrow();
            if st.wallpaper_preview.is_none() {
                return;
            }
            match st.wallpapers.get(i) {
                Some(p) if st.wallpaper.as_ref() != Some(p) => p.clone(),
                _ => return,
            }
        };
        let generation = {
            let mut st = self.st.borrow_mut();
            st.wallpaper_gen += 1;
            st.wallpaper_gen
        };
        let weak = self.weak.clone();
        Timer::single_shot(Duration::from_millis(150), move || {
            let Some(app) = weak.upgrade() else { return };
            {
                let mut st = app.st.borrow_mut();
                if st.wallpaper_gen != generation {
                    return; // still moving
                }
                match st.wallpaper_preview.as_mut() {
                    Some(p) => p.applied = true,
                    None => return, // confirmed or closed meanwhile
                }
            }
            app.apply_wallpaper(path, false);
        });
    }

    /// Esc / closing the launcher: back to the wallpaper and colours from before browsing.
    pub(super) fn wallpaper_preview_cancel(&self) {
        let Some(prev) = self.st.borrow_mut().wallpaper_preview.take() else { return };
        self.st.borrow_mut().wallpaper_gen += 1; // drop schemes still being generated
        if !prev.applied {
            return;
        }
        if self.cfg.borrow().bool("background.enabled") {
            if let Some(p) = &prev.original {
                system::set_wallpaper_async(p.clone());
            }
        }
        self.st.borrow_mut().wallpaper = prev.original;
        *self.scheme.borrow_mut() = prev.scheme;
        self.mark_current_wallpaper();
        self.apply_config();
    }

    pub(super) fn open_wallpaper_picker(&self) {
        if !self.st.borrow().drawers[Drawer::Launcher as usize].open {
            self.open(Drawer::Launcher, false);
        }
        let walls = list_wallpapers(&self.cfg.borrow());
        let current = self.st.borrow().wallpaper.clone();
        let sel = current.as_ref().and_then(|c| walls.iter().position(|w| w == c)).unwrap_or(0);
        // Snapshot for Esc before the selection starts previewing.
        self.st.borrow_mut().wallpaper_preview = Some(WallpaperPreview { original: current.clone(), scheme: self.scheme.borrow().clone(), applied: false });
        let model: Vec<WallpaperEntry> = walls
            .iter()
            .map(|p| WallpaperEntry {
                path: p.display().to_string().into(),
                name: p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default().into(),
                thumb: Image::default(),
                current: Some(p) == current.as_ref(),
            })
            .collect();
        let model = Rc::new(VecModel::from(model));
        self.launcher.set_wallpapers(ModelRc::from(model.clone()));
        self.launcher.set_wallpaper_mode(true);
        self.launcher.set_selected(sel as i32);
        self.st.borrow_mut().wallpapers = walls.clone();

        // Thumbnails come from the shell's thumbnail cache, off the UI thread,
        // and only around the selection: folders can hold thousands of images.
        const AHEAD: usize = 10;
        const KEEP: usize = 40;
        let px = (220.0 * self.primary_scale()) as i32;
        let weak_model = Rc::downgrade(&model);
        let launcher = self.launcher.as_weak();
        let (req_tx, req_rx) = crossbeam_channel::unbounded::<(usize, PathBuf)>();
        let (tx, rx) = crossbeam_channel::unbounded::<(usize, slint::SharedPixelBuffer<slint::Rgba8Pixel>)>();
        std::thread::spawn(move || {
            crate::platform::com_init(true);
            for (i, p) in req_rx {
                if let Some(buf) = path_thumbnail(&p, px, false) {
                    if tx.send((i, buf)).is_err() {
                        return;
                    }
                    post(|_| {}); // wake the UI loop
                }
            }
        });
        let mut requested = std::collections::HashSet::new();
        self.thumb_timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
            let Some(model) = weak_model.upgrade() else { return };
            use slint::Model;
            let sel = launcher.upgrade().map(|l| l.get_selected().max(0) as usize).unwrap_or(0);
            for i in sel.saturating_sub(AHEAD)..(sel + AHEAD + 1).min(walls.len()) {
                if requested.insert(i) {
                    let _ = req_tx.send((i, walls[i].clone()));
                }
            }
            requested.retain(|&i| {
                let keep = i.abs_diff(sel) <= KEEP;
                if !keep {
                    if let Some(mut e) = model.row_data(i) {
                        e.thumb = Image::default();
                        model.set_row_data(i, e);
                    }
                }
                keep
            });
            for (i, buf) in rx.try_iter() {
                if !requested.contains(&i) {
                    continue; // scrolled away meanwhile
                }
                if let Some(mut e) = model.row_data(i) {
                    e.thumb = Image::from_rgba8_premultiplied(buf);
                    model.set_row_data(i, e);
                }
            }
        });
    }

    pub(super) fn update_wallpaper_preview(&self) {
        let Some(path) = self.st.borrow().wallpaper.clone() else { return };
        let px = (320.0 * self.primary_scale()) as i32;
        // Shell thumbnails are cached by Explorer and decode quickly.
        let buf = path_thumbnail(&path, px, false);
        self.settings.set_has_wallpaper_preview(buf.is_some());
        if let Some(buf) = buf {
            self.settings.set_wallpaper_preview(Image::from_rgba8_premultiplied(buf));
        }
    }

    pub(super) fn random_wallpaper(&self) {
        let walls = list_wallpapers(&self.cfg.borrow());
        if walls.is_empty() {
            return;
        }
        let current = self.st.borrow().wallpaper.clone();
        let seed = chrono::Utc::now().timestamp_subsec_nanos() as usize ^ std::process::id() as usize;
        let mut i = seed % walls.len();
        if walls.len() > 1 && Some(&walls[i]) == current.as_ref() {
            i = (i + 1) % walls.len();
        }
        self.set_wallpaper(walls[i].clone());
    }

    /// Wallpaper grid in Settings, one page at a time (a big folder would
    /// otherwise hold hundreds of MB of thumbnails); thumbnails stream in off-thread.
    pub(super) fn refresh_settings_wallpapers(&self) {
        let walls = list_wallpapers(&self.cfg.borrow());
        let limit = self.st.borrow().wallpaper_limit;
        self.settings.set_wallpaper_dir(wallpaper_dir(&self.cfg.borrow()).display().to_string().into());
        self.settings.set_wallpapers_total(walls.len() as i32);
        let model = Rc::new(VecModel::from(walls.iter().take(limit).map(|p| self.wallpaper_entry(p)).collect::<Vec<_>>()));
        self.settings.set_wallpapers(ModelRc::from(model.clone()));
        self.stream_settings_thumbs(model);
    }

    /// "Show more": appends the next page without reloading loaded thumbnails.
    pub(super) fn more_settings_wallpapers(&self) {
        let limit = {
            let mut st = self.st.borrow_mut();
            st.wallpaper_limit += WALLPAPER_PAGE;
            st.wallpaper_limit
        };
        let walls = list_wallpapers(&self.cfg.borrow());
        self.settings.set_wallpapers_total(walls.len() as i32);
        let model = self.settings.get_wallpapers();
        let Some(vec) = model.as_any().downcast_ref::<VecModel<WallpaperEntry>>() else { return self.refresh_settings_wallpapers() };
        use slint::Model;
        for p in walls.iter().take(limit).skip(vec.row_count()) {
            vec.push(self.wallpaper_entry(p));
        }
        self.stream_settings_thumbs_model(model);
    }

    pub(super) fn wallpaper_entry(&self, p: &Path) -> WallpaperEntry {
        let current = self.st.borrow().wallpaper.as_deref() == Some(p);
        WallpaperEntry {
            path: p.display().to_string().into(),
            name: p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default().into(),
            thumb: Image::default(),
            current,
        }
    }

    pub(super) fn stream_settings_thumbs(&self, model: Rc<VecModel<WallpaperEntry>>) {
        self.stream_settings_thumbs_model(ModelRc::from(model));
    }

    /// Loads thumbnails for the grid rows that don't have one yet.
    pub(super) fn stream_settings_thumbs_model(&self, model: ModelRc<WallpaperEntry>) {
        use slint::Model;
        let jobs: Vec<(usize, PathBuf)> = (0..model.row_count())
            .filter_map(|i| model.row_data(i).filter(|e| e.thumb.size().width == 0).map(|e| (i, PathBuf::from(e.path.as_str()))))
            .collect();
        let px = (200.0 * self.primary_scale()) as i32;
        let (tx, rx) = crossbeam_channel::unbounded::<(usize, slint::SharedPixelBuffer<slint::Rgba8Pixel>)>();
        std::thread::spawn(move || {
            crate::platform::com_init(true);
            for (i, p) in jobs {
                if let Some(buf) = path_thumbnail(&p, px, false) {
                    if tx.send((i, buf)).is_err() {
                        return;
                    }
                }
            }
        });
        // Replacing the timer drops the previous receiver, which stops its loader.
        self.settings_thumb_timer.start(TimerMode::Repeated, Duration::from_millis(60), move || {
            for (i, buf) in rx.try_iter() {
                if let Some(mut e) = model.row_data(i) {
                    e.thumb = Image::from_rgba8_premultiplied(buf);
                    model.set_row_data(i, e);
                }
            }
        });
    }

    /// Downloads the images of a GitHub repository (Settings › Wallpaper) into
    /// `<wallpaperDir>/<repo>`; the URL is remembered in `paths.wallpaperRepo`.
    pub(super) fn start_wallpaper_download(&self, url: &str) {
        let tr = crate::i18n::tr;
        if self.st.borrow().wallpaper_dl.is_some() {
            return;
        }
        let g = self.settings.global::<ShellGlobal>();
        let Some(repo) = wallpaper_repo::Repo::parse(url) else {
            g.set_wallpaper_dl_state("error".into());
            g.set_wallpaper_dl_text(tr("Not a GitHub repository link").into());
            return;
        };
        if self.cfg.borrow().str("paths.wallpaperRepo") != url.trim() {
            self.set_config("paths.wallpaperRepo", &serde_json::to_string(url.trim()).unwrap_or_default());
        }
        // The configured folder itself (never the Windows fallback).
        let dest = self.cfg.borrow().path("paths.wallpaperDir").join(&repo.name);
        log::info!("downloading wallpapers from {}/{} into {}", repo.owner, repo.name, dest.display());
        g.set_wallpaper_dl_state("indexing".into());
        g.set_wallpaper_dl_progress(0.0);
        g.set_wallpaper_dl_text(tr("Reading the repository…").into());
        let dl = wallpaper_repo::start(repo, dest, |p| post(move |app| app.on_wallpaper_download(p)));
        let mut st = self.st.borrow_mut();
        st.wallpaper_dl = Some(dl);
        st.wallpaper_dl_shown = 0;
    }

    pub(super) fn on_wallpaper_download(&self, p: DlProgress) {
        let tr = crate::i18n::tr;
        let es = crate::i18n::is_es();
        let g = self.settings.global::<ShellGlobal>();
        match p {
            DlProgress::Indexing => {}
            DlProgress::Running { done, total, bytes, total_bytes } => {
                g.set_wallpaper_dl_state("running".into());
                g.set_wallpaper_dl_progress(if total_bytes > 0 { bytes as f32 / total_bytes as f32 } else { done as f32 / total.max(1) as f32 });
                let sizes = format!("{} / {}", size_label(bytes), size_label(total_bytes));
                g.set_wallpaper_dl_text(if es { format!("{done} de {total} imágenes · {sizes}") } else { format!("{done} of {total} images · {sizes}") }.into());
                // Show the first images quickly, then refresh the grid now and then.
                let shown = self.st.borrow().wallpaper_dl_shown;
                if (shown == 0 && done >= 8) || done >= shown + 150 {
                    self.st.borrow_mut().wallpaper_dl_shown = done;
                    self.refresh_settings_wallpapers();
                }
            }
            DlProgress::Finished { downloaded, skipped, failed, cancelled } => {
                self.st.borrow_mut().wallpaper_dl = None;
                g.set_wallpaper_dl_state("done".into());
                g.set_wallpaper_dl_progress(1.0);
                let mut text = match (cancelled, es) {
                    (true, true) => format!("Cancelado · {downloaded} descargadas"),
                    (true, false) => format!("Cancelled · {downloaded} downloaded"),
                    (false, true) => format!("Listo · {downloaded} nuevas, {skipped} ya estaban"),
                    (false, false) => format!("Done · {downloaded} new, {skipped} already there"),
                };
                if failed > 0 {
                    text += &if es { format!(" · {failed} con error") } else { format!(" · {failed} failed") };
                }
                g.set_wallpaper_dl_text(text.into());
                self.refresh_settings_wallpapers();
            }
            DlProgress::Failed(e) => {
                self.st.borrow_mut().wallpaper_dl = None;
                g.set_wallpaper_dl_state("error".into());
                g.set_wallpaper_dl_text(format!("{}: {}", tr("Error"), tr(&e)).into());
            }
        }
    }

    /// Marks the current wallpaper in the settings grid without reloading thumbnails.
    pub(super) fn mark_current_wallpaper(&self) {
        use slint::Model;
        let current = self.st.borrow().wallpaper.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
        let model = self.settings.get_wallpapers();
        for i in 0..model.row_count() {
            if let Some(mut e) = model.row_data(i) {
                let is = e.path.as_str() == current;
                if e.current != is {
                    e.current = is;
                    model.set_row_data(i, e);
                }
            }
        }
    }
}
