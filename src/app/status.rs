//! Periodic refreshers and service updates: clock, status icons, recordings, system stats, media, weather, network and devices.

use super::*;

impl Caelestia {
    pub(super) fn tick_clock_force(&self) {
        self.st.borrow_mut().clock_key.clear();
        self.tick_clock();
    }

    pub(super) fn tick_clock(&self) {
        let now = Local::now();
        let twelve = self.cfg.borrow().bool("services.useTwelveHourClock");
        let key = format!("{}{twelve}", now.format("%Y%m%d%H%M"));
        if self.st.borrow().clock_key == key {
            return;
        }
        self.st.borrow_mut().clock_key = key;
        let hours = if twelve { now.format("%I").to_string() } else { now.format("%H").to_string() };
        let minutes = now.format("%M").to_string();
        let ampm = if twelve { now.format("%P").to_string() } else { String::new() };
        let shown = add_months(now.date_naive(), self.st.borrow().calendar_offset);
        let (cal_title, cal) = calendar(shown, now.date_naive());
        let cal = ModelRc::new(VecModel::from(cal));
        let screens = self.screens.borrow();
        let globals = screens.iter().map(|s| s.bar.global::<ShellGlobal>()).chain([self.dashboard.global::<ShellGlobal>(), self.popout.global::<ShellGlobal>()]);
        for g in globals {
            g.set_hours(hours.clone().into());
            g.set_minutes(minutes.clone().into());
            g.set_ampm(ampm.clone().into());
            let loc = crate::i18n::chrono_locale();
            let fmt = |f: &str| crate::i18n::capitalize(&now.format_localized(f, loc).to_string());
            g.set_date_long(fmt(if crate::i18n::is_es() { "%A, %d de %B" } else { "%A, %d %B" }).into());
            g.set_date_short(fmt("%a %d %b").into());
            g.set_weekday(fmt("%A").into());
        }
        drop(screens);
        for g in [self.dashboard.global::<ShellGlobal>(), self.popout.global::<ShellGlobal>()] {
            g.set_calendar_title(cal_title.clone().into());
            g.set_calendar(cal.clone());
        }
        if !self.st.borrow().notifs.is_empty() {
            self.push_notifications(); // "2 min ago" labels
        }
        let g = self.dashboard.global::<ShellGlobal>();
        let mut user = g.get_user();
        user.uptime = crate::i18n::uptime(system::uptime_secs()).into();
        g.set_user(user);
    }

    pub(super) fn refresh_status(&self) {
        let audio = audio::state();
        let battery = system::battery();
        let net = self.st.borrow().net.clone();
        self.st.borrow_mut().audio = audio.clone();
        let status = Status {
            volume: audio.volume,
            muted: audio.muted,
            mic_volume: audio.mic_volume,
            mic_muted: audio.mic_muted,
            online: net.online,
            wifi: net.wifi,
            wifi_strength: net.signal.clamp(0, 4).min(3),
            network_name: net.name.into(),
            bluetooth: net.bt_radio.unwrap_or(false),
            bluetooth_available: net.bt_radio.is_some(),
            wifi_available: net.wifi_radio.unwrap_or(net.wifi),
            battery_present: battery.present,
            battery: battery.percent,
            charging: battery.charging,
        };
        let screens = self.screens.borrow();
        let globals = screens.iter().map(|s| s.bar.global::<ShellGlobal>()).chain([
            self.utilities.global::<ShellGlobal>(),
            self.settings.global::<ShellGlobal>(),
            self.popout.global::<ShellGlobal>(),
        ]);
        for g in globals {
            if g.get_status() != status {
                g.set_status(status.clone());
            }
        }
        drop(screens);
        // Enumerating endpoints is the costly part: only while a list is on screen.
        if self.audio_lists_visible() {
            self.push_audio_devices(false);
        }
    }

    /// The endpoint lists only appear in the audio popout and in Settings.
    pub(super) fn audio_lists_visible(&self) -> bool {
        let st = self.st.borrow();
        (st.popout.open && st.popout.kind == "audio") || st.settings_visible
    }

    pub(super) fn refresh_recordings(&self) {
        let dirs = {
            let cfg = self.cfg.borrow();
            vec![recordings_dir(&cfg), dirs::video_dir().unwrap_or_default().join("Captures")]
        };
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = dirs
            .iter()
            .filter_map(|d| std::fs::read_dir(d).ok())
            .flat_map(|rd| rd.flatten())
            .map(|e| e.path())
            .filter(|p| matches!(p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("mp4" | "mkv" | "webm")))
            .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
            .collect();
        files.sort_by(|a, b| b.0.cmp(&a.0));
        let recs: Vec<Recording> = files
            .into_iter()
            .take(4)
            .map(|(t, p)| {
                let when: chrono::DateTime<Local> = t.into();
                let stamp = when.format("%d/%m/%y %-I:%M%P");
                let name = if crate::i18n::is_es() { format!("Grabación del {stamp}") } else { format!("Recording at {stamp}") };
                Recording { name: name.into(), path: p.display().to_string().into() }
            })
            .collect();
        self.utilities.global::<ShellGlobal>().set_recordings(ModelRc::new(VecModel::from(recs)));
    }

    pub(super) fn on_sys(&self, s: SysSnapshot) {
        let stats = SysStats {
            cpu: s.cpu,
            cpu_name: s.cpu_name.into(),
            cpu_temp: s.cpu_temp.map(|t| format!("{t:.0}°C")).unwrap_or_else(|| "N/A".into()).into(),
            cpu_temp_help: s.cpu_temp_help.into(),
            gpu: s.gpu.unwrap_or(0.0),
            gpu_name: if s.gpu_name.is_empty() { "Unknown".into() } else { s.gpu_name.into() },
            gpu_temp: s.gpu_temp.map(|t| format!("{t:.0}°C")).unwrap_or_else(|| "N/A".into()).into(),
            has_gpu: s.gpu.is_some(),
            mem_used: fmt_gib(s.mem_used).into(),
            mem_total: format!("{} GiB", fmt_gib(s.mem_total)).into(),
            mem: if s.mem_total > 0 { s.mem_used as f32 / s.mem_total as f32 } else { 0.0 },
            disk_name: s.disk_name.into(),
            disk_used: fmt_gib(s.disk_used).into(),
            disk_total: format!("{} GiB", fmt_gib(s.disk_total)).into(),
            disk_count: s.disk_count as i32,
            disk: if s.disk_total > 0 { s.disk_used as f32 / s.disk_total as f32 } else { 0.0 },
            net_down: format!("{}/s", fmt_bytes(s.down_rate)).into(),
            net_up: format!("{}/s", fmt_bytes(s.up_rate)).into(),
            net_total_down: fmt_bytes(s.total_down as f64).into(),
            net_total_up: fmt_bytes(s.total_up as f64).into(),
            net_down_path: s.down_path.into(),
            net_up_path: s.up_path.into(),
        };
        self.dashboard.global::<ShellGlobal>().set_sys(stats);
    }

    pub(super) fn on_media(&self, m: MediaSnapshot) {
        let g = self.dashboard.global::<ShellGlobal>();
        let prev = g.get_media();
        let (cover, has_cover) = match m.cover {
            Some(Some(buf)) => (Image::from_rgba8(buf), true),
            Some(None) => (Image::default(), false),
            None => (prev.cover.clone(), prev.has_cover),
        };
        g.set_media(MediaInfo {
            available: m.available,
            playing: m.playing,
            title: m.title.into(),
            artist: m.artist.into(),
            album: m.album.into(),
            app: m.app.into(),
            position: m.position as f32,
            length: m.length as f32,
            position_text: fmt_time(m.position).into(),
            length_text: fmt_time(m.length).into(),
            cover,
            has_cover,
        });
    }

    pub(super) fn on_weather(&self, w: Option<WeatherSnapshot>) {
        let status = if w.is_some() { "ok" } else { "error" };
        for g in [self.dashboard.global::<ShellGlobal>(), self.settings.global::<ShellGlobal>()] {
            g.set_weather_status(status.into());
        }
        let weather = match w {
            Some(w) => Weather {
                available: true,
                city: w.city.into(),
                temp: w.temp.into(),
                feels: w.feels.into(),
                icon: w.icon.into(),
                description: w.description.into(),
                humidity: w.humidity.into(),
                wind: w.wind.into(),
                precipitation: w.precipitation.into(),
                sunrise: w.sunrise.into(),
                sunset: w.sunset.into(),
                forecast: ModelRc::new(VecModel::from(
                    w.forecast.into_iter().map(|d| ForecastDay { day: d.day.into(), icon: d.icon.into(), max: d.max.into(), min: d.min.into() }).collect::<Vec<_>>(),
                )),
            },
            None => Weather::default(),
        };
        self.settings.global::<ShellGlobal>().set_weather(weather.clone());
        self.dashboard.global::<ShellGlobal>().set_weather(weather);
    }

    /// Live city search: waits 350 ms after the last keystroke, then queries
    /// off-thread; stale responses (older generations) are dropped.
    pub(super) fn place_search(&self, text: &str) {
        let generation = {
            let mut st = self.st.borrow_mut();
            st.place_gen += 1;
            st.place_gen
        };
        let g = self.settings.global::<ShellGlobal>();
        if text.trim().chars().count() < 2 {
            self.st.borrow_mut().places.clear();
            g.set_places(ModelRc::default());
            g.set_place_status("idle".into());
            return;
        }
        g.set_place_status("searching".into());
        let text = text.to_owned();
        let weak = self.weak.clone();
        Timer::single_shot(Duration::from_millis(350), move || {
            let Some(app) = weak.upgrade() else { return };
            if app.st.borrow().place_gen != generation {
                return; // user kept typing
            }
            let (lang, region) = (system_lang(), system_region());
            std::thread::spawn(move || {
                let result = crate::services::weather::search_places(&text, &lang, &region);
                post(move |app| {
                    if app.st.borrow().place_gen != generation {
                        return;
                    }
                    let g = app.settings.global::<ShellGlobal>();
                    match result {
                        Ok(places) => {
                            let model: Vec<Place> = places
                                .iter()
                                .map(|p| Place { name: p.name.clone().into(), detail: p.detail.clone().into(), country_code: p.country_code.clone().into(), coords: p.coords().into() })
                                .collect();
                            g.set_place_status(if places.is_empty() { "none" } else { "idle" }.into());
                            g.set_places(ModelRc::new(VecModel::from(model)));
                            app.st.borrow_mut().places = places;
                        }
                        Err(e) => {
                            log::warn!("place search failed: {e:#}");
                            g.set_place_status("error".into());
                        }
                    }
                });
            });
        });
    }

    /// Saves the chosen place (or `None` = automatic/IP) and refetches weather.
    pub(super) fn set_weather_place(&self, place: Option<crate::services::weather::Place>) {
        let (loc, name) = match &place {
            Some(p) => (p.coords(), p.label()),
            None => (String::new(), String::new()),
        };
        let result = {
            let mut cfg = self.cfg.borrow_mut();
            cfg.set("services.weatherLocation", Value::String(loc))
                .and_then(|_| cfg.set("services.weatherLocationName", Value::String(name)))
                .and_then(|_| cfg.save())
        };
        if let Err(e) = result {
            log::error!("saving weather location: {e:#}");
        }
        {
            let mut st = self.st.borrow_mut();
            st.places.clear();
            st.place_gen += 1;
        }
        let g = self.settings.global::<ShellGlobal>();
        g.set_places(ModelRc::default());
        g.set_place_status("idle".into());
        for g in [self.dashboard.global::<ShellGlobal>(), self.settings.global::<ShellGlobal>()] {
            g.set_weather_status("loading".into());
        }
        self.apply_config(); // new query -> weather service refetches
    }

    /// Mirrors Windows' DND state (it can also change from the Action Center).
    pub(super) fn refresh_dnd(&self) {
        if let Some(on) = crate::platform::focus::dnd_enabled() {
            self.st.borrow_mut().dnd = on;
            for g in [self.utilities.global::<ShellGlobal>(), self.popout.global::<ShellGlobal>()] {
                if g.get_dnd() != on {
                    g.set_dnd(on);
                }
            }
        }
    }

    pub(super) fn on_net(&self, n: NetSnapshot) {
        self.st.borrow_mut().net = n;
        self.refresh_status();
        self.push_bt_devices();
        self.push_wifi_networks();
    }

    /// Wi-Fi networks -> the network popout and Settings › Network.
    pub(super) fn push_wifi_networks(&self) {
        let tr = crate::i18n::tr;
        let (networks, limited, wrong_password) = {
            let st = self.st.borrow();
            let n = &st.net;
            let model: Vec<UiWifiNet> = n
                .wifi_networks
                .iter()
                .map(|w| {
                    let error = n.wifi_error.as_ref().filter(|(ssid, _)| ssid == &w.ssid).map(|(_, e)| tr(e.message())).unwrap_or_default();
                    UiWifiNet {
                        ssid: w.ssid.clone().into(),
                        // Saved networks listed without a scan have no signal.
                        bars: if n.wifi_denied { -1 } else { w.bars as i32 },
                        secured: w.secured,
                        connected: w.connected,
                        saved: w.saved,
                        busy: n.wifi_busy == w.ssid,
                        error: error.into(),
                    }
                })
                .collect();
            let wrong_password = n.wifi_error.as_ref().filter(|(_, e)| *e == crate::platform::wifi::ConnectError::Password).map(|(s, _)| s.clone());
            (ModelRc::new(VecModel::from(model)), n.wifi_denied, wrong_password)
        };
        for g in [self.popout.global::<ShellGlobal>(), self.settings.global::<ShellGlobal>()] {
            g.set_wifi_networks(networks.clone());
            g.set_wifi_limited(limited);
        }
        // A rejected password: ask again in Settings.
        if let Some(ssid) = wrong_password {
            self.settings.global::<ShellGlobal>().set_wifi_ask(ssid.into());
        }
    }

    /// Paired Bluetooth devices -> UI, with a "busy" flag while a
    /// (dis)connection is in flight (until the state flips or 10 s pass).
    pub(super) fn push_bt_devices(&self) {
        let devices = self.st.borrow().net.bt_devices.clone();
        let model: Vec<UiBtDevice> = {
            let mut st = self.st.borrow_mut();
            st.bt_busy.retain(|id, (t, was)| t.elapsed() < Duration::from_secs(10) && devices.iter().find(|d| &d.id == id).is_some_and(|d| d.connected == *was));
            devices
                .iter()
                .map(|d| UiBtDevice {
                    id: d.id.clone().into(),
                    name: d.name.clone().into(),
                    icon: d.icon.into(),
                    connected: d.connected,
                    audio: d.audio,
                    busy: st.bt_busy.contains_key(&d.id),
                })
                .collect()
        };
        let model = ModelRc::new(VecModel::from(model));
        for g in [self.popout.global::<ShellGlobal>(), self.settings.global::<ShellGlobal>()] {
            g.set_bt_devices(model.clone());
        }
    }

    /// Audio endpoint lists -> UI (only when they change), at most once a
    /// second unless `force`.
    pub(super) fn push_audio_devices(&self, force: bool) {
        if !force && self.st.borrow().audio_lists_at.is_some_and(|t| t.elapsed() < Duration::from_millis(900)) {
            return;
        }
        self.st.borrow_mut().audio_lists_at = Some(Instant::now());
        let lists = (audio::devices(false), audio::devices(true));
        if self.st.borrow().audio_lists == lists {
            return;
        }
        let conv = |v: &[audio::AudioDevice]| {
            ModelRc::new(VecModel::from(v.iter().map(|d| UiAudioDevice { id: d.id.clone().into(), name: d.name.clone().into(), default: d.default }).collect::<Vec<_>>()))
        };
        let (outs, ins) = (conv(&lists.0), conv(&lists.1));
        for g in [self.popout.global::<ShellGlobal>(), self.settings.global::<ShellGlobal>()] {
            g.set_audio_outputs(outs.clone());
            g.set_audio_inputs(ins.clone());
        }
        self.st.borrow_mut().audio_lists = lists;
    }
}
