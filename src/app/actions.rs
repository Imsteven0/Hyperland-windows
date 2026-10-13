//! The action bus: everything the UI, IPC and hotkeys can ask for.

use super::*;

impl Caelestia {
    pub(super) fn on_ipc(&self, cmd: &str) {
        match cmd {
            "quit" => {
                let _ = slint::quit_event_loop();
            }
            "reload" => self.action("reload-config", ""),
            other => {
                if let Some(target) = other.strip_prefix("toggle:") {
                    self.action("toggle", target);
                } else if let Some(kind) = other.strip_prefix("popout:") {
                    // Centre it on the bar's status area of the active screen.
                    let (area, sc) = self.screen_geom(self.active_screen());
                    let y = (area.height() as f32 / sc) * 0.8;
                    self.show_popout(&format!("{kind}:{y}"));
                    self.st.borrow_mut().popout.item_hovered = true;
                } else if let Some(kind) = other.strip_prefix("picker:") {
                    self.open_picker(match kind {
                        "variant" => "variant",
                        "opacity" => "opacity",
                        _ => "scheme",
                    });
                } else if let Some(mode) = other.strip_prefix("launcher:") {
                    self.open_launcher_mode(match mode {
                        "calc" => LauncherMode::Calc,
                        "emoji" => LauncherMode::Emoji,
                        _ => LauncherMode::Clipboard,
                    });
                } else if let Some(page) = other.strip_prefix("settings:") {
                    self.action("settings-page", page);
                } else if let Some(tab) = other.strip_prefix("dashboard:").and_then(|t| t.parse::<i32>().ok()) {
                    self.dashboard.set_tab(tab);
                    if !self.st.borrow().drawers[Drawer::Dashboard as usize].open {
                        self.open(Drawer::Dashboard, false);
                    }
                }
            }
        }
    }

    pub(super) fn on_hotkey(&self, target: &str) {
        if let Some(cmd) = target.strip_prefix("komorebi:") {
            if self.komorebi_active() {
                komorebi::command_line(cmd);
            }
            return;
        }
        match target {
            "settings" => self.toggle_settings(None),
            "calc" => self.open_launcher_mode(LauncherMode::Calc),
            "emoji" => self.open_launcher_mode(LauncherMode::Emoji),
            "clipboard" => self.open_launcher_mode(LauncherMode::Clipboard),
            t => {
                if let Some(d) = Drawer::parse(t) {
                    self.toggle(d);
                }
            }
        }
    }

    pub(super) fn action(&self, id: &str, arg: &str) {
        match id {
            "toggle" => match arg {
                "settings" => self.toggle_settings(None),
                other => {
                    if let Some(d) = Drawer::parse(other) {
                        self.toggle(d);
                    }
                }
            },
            "close" => {
                if let Some(d) = Drawer::parse(arg) {
                    self.close(d);
                }
            }
            "popout" => self.show_popout(arg),
            "cpu-temp" => match crate::services::temps::lhm_path() {
                // LibreHardwareMonitor needs admin rights to load its sensor driver.
                Some(exe) => unsafe {
                    use windows::{Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWMINNOACTIVE}, core::{HSTRING, PCWSTR}};
                    let _ = ShellExecuteW(None, &HSTRING::from("runas"), &HSTRING::from(exe.as_os_str()), PCWSTR::null(), PCWSTR::null(), SW_SHOWMINNOACTIVE);
                },
                None => system::spawn(&["cmd".into(), "/c".into(), "start".into(), "cmd".into(), "/k".into(), "winget install --id LibreHardwareMonitor.LibreHardwareMonitor -e".into()]),
            },
            "calendar" => {
                let off = match arg {
                    "0" => 0,
                    a => self.st.borrow().calendar_offset + a.parse::<i32>().unwrap_or(0),
                };
                self.st.borrow_mut().calendar_offset = off.clamp(-1200, 1200);
                self.tick_clock_force();
            }
            "media-seek" => {
                if let (Ok(f), Some(svc)) = (parse_num::<f64>(arg).ok_or(()), self.svc.borrow().as_ref()) {
                    svc.media.send(MediaCommand::Seek(f));
                }
            }
            "disk-next" => {
                if let Some(svc) = self.svc.borrow().as_ref() {
                    let cur = svc.sysmon.disk_index.load(Ordering::Relaxed);
                    let count = self.dashboard.global::<ShellGlobal>().get_sys().disk_count.max(1) as u64;
                    // From "system drive" start at the next real index.
                    let next = if cur == u64::MAX { 1 } else { (cur + 1) % count };
                    svc.sysmon.disk_index.store(next, Ordering::Relaxed);
                }
            }
            "weather-refresh" => {
                let q = weather_query(&self.cfg.borrow());
                self.dashboard.global::<ShellGlobal>().set_weather_status("loading".into());
                if let Some(svc) = self.svc.borrow().as_ref() {
                    svc.weather.refresh(q);
                }
            }
            "komorebi-start" => {
                    self.st.borrow_mut().komorebi_colours.clear();
                self.push_komorebi_theme();
                komorebi::start();
            }
            "komorebi-stop" => komorebi::stop(),
            "komorebi-retile" => komorebi::command(&["retile"]),
            "komorebi-install" => system::spawn(&["cmd".into(), "/c".into(), "start".into(), "cmd".into(), "/k".into(), "winget install --id LGUG2Z.komorebi -e".into()]),
            "komorebi-config" => system::reveal(&komorebi::config_path()),
            "set-scheme" => self.apply_scheme_id(arg),
            "opacity-preset" => self.opacity_preset(arg),
            "set-wallpaper" => self.set_wallpaper(PathBuf::from(arg)),
            "random-wallpaper" => self.random_wallpaper(),
            "wallpaper-download" => self.start_wallpaper_download(arg),
            "wallpaper-download-cancel" => {
                if let Some(dl) = self.st.borrow().wallpaper_dl.as_ref() {
                    dl.cancel();
                }
            }
            "wallpapers-more" => self.more_settings_wallpapers(),
            "choose-wallpaper-dir" => {
                if let Some(dir) = rfd::FileDialog::new().set_title("Wallpaper folder").pick_folder() {
                    self.set_config("paths.wallpaperDir", &serde_json::to_string(&dir.display().to_string()).unwrap_or_default());
                    self.refresh_settings_wallpapers();
                }
            }
            "popout-leave" => {
                let mut st = self.st.borrow_mut();
                if st.popout.kind == arg {
                    st.popout.item_hovered = false;
                }
            }
            "settings-page" => {
                self.close_popout();
                self.toggle_settings(Some(true));
                self.settings.set_page(arg.parse().unwrap_or(0));
            }
            "workspace" => {
                let target: usize = arg.parse().unwrap_or(1);
                let ds = self.st.borrow().desktops.clone();
                desktops::switch_to(target, ds.current, ds.occupied.len());
            }
            "workspace-relative" => {
                let ds = self.st.borrow().desktops.clone();
                let delta: i64 = arg.parse().unwrap_or(1);
                let target = (ds.current as i64 + delta).clamp(1, ds.occupied.len().max(1) as i64) as usize;
                desktops::switch_to(target, ds.current, ds.occupied.len());
            }
            "volume-step" => {
                let step: f32 = parse_num(arg).unwrap_or(0.05);
                let inc = self.cfg.borrow().f32("services.audioIncrement").max(0.01);
                let max = self.cfg.borrow().f32("services.maxVolume").clamp(0.0, 1.0);
                let cur = audio::state().volume;
                audio::set_volume((cur + step.signum() * inc).clamp(0.0, max));
                self.refresh_status();
            }
            "volume-set" => {
                audio::set_volume(parse_num(arg).unwrap_or(0.5));
                self.refresh_status();
            }
            "mic-volume-set" => {
                audio::set_mic_volume(parse_num(arg).unwrap_or(0.5));
                self.refresh_status();
            }
            "audio-default" => {
                if !audio::set_default(arg) {
                    log::warn!("could not set default audio device {arg}");
                }
                self.st.borrow_mut().audio_lists = Default::default();
                self.refresh_status();
            }
            "bt-connect" | "bt-disconnect" => {
                let connect = id == "bt-connect";
                self.st.borrow_mut().bt_busy.insert(arg.to_owned(), (Instant::now(), !connect));
                self.radio(RadioCmd::BtConnect(arg.to_owned(), connect));
                self.push_bt_devices();
            }
            "bt-forget" => self.radio(RadioCmd::BtForget(arg.to_owned())),
            "place-search" => self.place_search(arg),
            "dashboard-tab" => self.dashboard.set_tab(arg.parse().unwrap_or(0)),
            "place-select" => {
                let place = self.st.borrow().places.get(arg.parse::<usize>().unwrap_or(0)).cloned();
                if let Some(p) = place {
                    self.set_weather_place(Some(p));
                }
            }
            "place-detect" => {
                let g = self.settings.global::<ShellGlobal>();
                g.set_place_status("detecting".into());
                let lang = system_lang();
                std::thread::spawn(move || {
                    crate::platform::com_init(false);
                    let result = crate::services::weather::detect_place(&lang);
                    post(move |app| match result {
                        Ok(p) => app.set_weather_place(Some(p)),
                        Err(e) => {
                            log::warn!("location detection failed: {e:#}");
                            app.settings.global::<ShellGlobal>().set_place_status("error".into());
                        }
                    });
                });
            }
            "place-auto" => self.set_weather_place(None),
            "bt-refresh" => self.radio(RadioCmd::Refresh),
            "toggle-mic" => {
                let muted = audio::state().mic_muted;
                audio::set_muted(true, !muted);
                self.refresh_status();
            }
            "media" => {
                let cmd = match arg {
                    "next" => MediaCommand::Next,
                    "previous" => MediaCommand::Previous,
                    _ => MediaCommand::Toggle,
                };
                if let Some(svc) = self.svc.borrow().as_ref() {
                    svc.media.send(cmd);
                }
            }
            "keep-awake" => {
                let on = arg == "on";
                system::keep_awake(on);
                self.st.borrow_mut().keep_awake = on;
                self.utilities.global::<ShellGlobal>().set_keep_awake(on);
            }
            "toggle-dnd" => {
                // Real Windows Do Not Disturb (Quiet Hours profile), not just a flag.
                let current = crate::platform::focus::dnd_enabled().unwrap_or(self.st.borrow().dnd);
                if crate::platform::focus::set_dnd(!current) {
                    self.st.borrow_mut().dnd = !current;
                } else {
                    log::warn!("could not change Do Not Disturb");
                }
                self.refresh_dnd();
            }
            "toggle-wifi" => self.radio(RadioCmd::ToggleWifi),
            "toggle-bluetooth" => self.radio(RadioCmd::ToggleBluetooth),
            "screenshot" => {
                self.close(Drawer::Utilities);
                if arg == "fullscreen" {
                    win::send_keys(&[0x5B], &[0x2C]); // Win+PrtScn -> Pictures\Screenshots
                } else {
                    system::open("ms-screenclip:");
                }
            }
            "record" => {
                if arg == "folder" {
                    system::open(&recordings_dir(&self.cfg.borrow()).display().to_string());
                } else {
                    self.close(Drawer::Utilities);
                    win::send_keys(&[0x5B, 0x12], &[0x52]); // Win+Alt+R (Game Bar capture)
                }
            }
            "game-bar" => win::send_keys(&[0x5B], &[0x47]), // Win+G
            "open-path" => system::open(arg),
            "reveal-path" => system::reveal(Path::new(arg)),
            "delete-recording" => {
                system::recycle(Path::new(arg));
                self.refresh_recordings();
            }
            "session" => {
                self.close(Drawer::Session);
                system::session(arg);
            }
            "open-uri" => system::open(arg),
            "winget-upgrade" => system::spawn(&["cmd".into(), "/c".into(), "start".into(), "cmd".into(), "/k".into(), "winget upgrade --all".into()]),
            "open-config-dir" => {
                let dir = self.cfg.borrow().paths.config_dir.join(arg);
                let _ = std::fs::create_dir_all(&dir);
                system::open(&dir.display().to_string());
            }
            "reload-config" => {
                let _ = self.cfg.borrow_mut().reload();
                if let Ok(s) = Scheme::load(&self.cfg.borrow().paths.scheme_json) {
                    *self.scheme.borrow_mut() = s;
                }
                self.apply_config();
                self.load_plugins();
            }
            "notif-open" => self.open_notification(arg.parse().unwrap_or(0)),
            "notif-dismiss" => self.dismiss_notifications(|n| n.id.to_string() == arg),
            "notif-dismiss-group" => self.dismiss_notifications(|n| n.app_id == arg),
            "notif-clear" => {
                if let Some(svc) = self.svc.borrow().as_ref() {
                    svc.notifications.send(NotifCommand::Clear);
                }
                self.st.borrow_mut().notifs.clear();
                self.push_notifications();
            }
            "notif-expand" => {
                {
                    let mut st = self.st.borrow_mut();
                    if !st.notif_expanded.remove(arg) {
                        st.notif_expanded.insert(arg.to_owned());
                    }
                }
                self.push_notifications();
            }
            "plugin-click" => self.plugin_click(arg.parse().unwrap_or(usize::MAX)),
            "plugin-toggle" => self.toggle_plugin(arg),
            "plugins-reload" => self.load_plugins(),
            "dots-fetch" => self.fetch_dots(arg),
            "dots-apply" => self.apply_dots(arg == "wallpapers"),
            "dots-undo" => self.undo_dots(),
            "dots-export" => self.export_dots(),
            "dots-import" => self.import_dots(),
            "open-wallpapers" => {
                self.toggle_settings(Some(false));
                self.open_wallpaper_picker();
            }
            other => log::debug!("unhandled action {other}({arg})"),
        }
    }

    pub(super) fn radio(&self, cmd: RadioCmd) {
        if let Some(svc) = self.svc.borrow().as_ref() {
            svc.net.send(cmd);
        }
    }
}
