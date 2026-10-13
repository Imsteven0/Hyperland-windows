//! The shell controller. Owns every Slint window, the config/scheme, the
//! background services and the drawer state machine. Everything here runs on
//! the UI thread; services talk to it through `invoke_from_event_loop`.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use anyhow::Result;
use chrono::{Datelike, Local, NaiveDate};
use serde_json::Value;
use slint::{ComponentHandle, Image, ModelRc, PhysicalPosition, SharedString, Timer, TimerMode, VecModel};

use crate::{
    AppEntry, AudioDevice as UiAudioDevice, PickerEntry, Place, BarWindow, BtDevice as UiBtDevice, BorderWindow, CalendarDay, DashboardWindow, ForecastDay, InfoRow, LauncherWindow, MediaInfo, PopoutWindow, Recording, SessionWindow,
    SchemeCard, SettingsWindow, Shell as ShellGlobal, Status, SysStats, Theme, Tokens, UiConfig, UserInfo, UtilitiesWindow, WallpaperEntry, Weather, Workspace,
    config::{Config, Paths, watch::{Changed, Watcher}},
    hotkeys::Hotkeys,
    platform::{
        audio::{self, AudioState},
        desktops::{self, Desktops},
        shell_items::{AppRecord, path_thumbnail},
        system,
        window::{self as win, AppBar, Edge as AbEdge, Monitor, Rect, SurfaceStyle},
    },
    services::{
        apps::{ACTIONS, Action, Fuzzy, Mode as LauncherMode, Prefixes, spawn_index},
        calc::{self, CalcError},
        clipboard::ClipHistory,
        emoji,
        connectivity::{Connectivity, NetSnapshot, RadioCmd},
        icons::{self, IconCache},
        komorebi::{self, KState, KStatus},
        media::{Media, MediaCommand, MediaSnapshot, fmt_time},
        perf::{self, Reason as PerfReason},
        sysmon::{SysMonitor, SysSnapshot, fmt_bytes, fmt_gib},
        wallpaper_repo::{self, Download, Progress as DlProgress},
        weather::{WeatherQuery, WeatherService, WeatherSnapshot},
    },
    theme::{Scheme, source_from_image},
};

// Applies `$body` to every top-level window (each has its own copy of globals),
// including the per-monitor bars and frames.
macro_rules! each_window {
    ($app:expr, $w:ident => $body:expr) => {{
        for s in $app.screens.borrow().iter() {
            { let $w = &s.bar; $body; }
            { let $w = &s.border; $body; }
        }
        { let $w = &$app.popout; $body; }
        { let $w = &$app.dashboard; $body; }
        { let $w = &$app.launcher; $body; }
        { let $w = &$app.utilities; $body; }
        { let $w = &$app.session; $body; }
        { let $w = &$app.settings; $body; }
    }};
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drawer {
    Dashboard = 0,
    Launcher = 1,
    Utilities = 2,
    Session = 3,
}

impl Drawer {
    const ALL: [Drawer; 4] = [Drawer::Dashboard, Drawer::Launcher, Drawer::Utilities, Drawer::Session];
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "dashboard" => Self::Dashboard,
            "launcher" => Self::Launcher,
            "utilities" => Self::Utilities,
            "session" => Self::Session,
            _ => return None,
        })
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct DrawerState {
    open: bool,
    by_hover: bool,
    rect: Option<Rect>,
    generation: u32,
    /// Index into `screens` the drawer is shown on.
    screen: usize,
}

#[derive(Debug, Default, Clone)]
struct PopoutState {
    open: bool,
    kind: String,
    /// The bar item that opened it is still under the pointer.
    item_hovered: bool,
    rect: Option<Rect>,
    generation: u32,
}

/// One monitor: its own bar, frame and reserved edges (Caelestia draws the
/// shell on every screen).
struct Screen {
    monitor: Monitor,
    area: Rect,
    bar: BarWindow,
    border: BorderWindow,
    appbars: Vec<AppBar>,
    styled: bool,
}

struct AppItem {
    name: String,
    description: String,
    launch: String,
    icon: Option<Image>,
}

enum Hit {
    App(usize),
    Action(Action),
    /// Calculator result or emoji: Enter copies it.
    Copy(String),
    /// Clipboard history entry (index into `State::clips`).
    Clip(usize),
    /// Informational row (empty calculator, no matches...).
    Hint,
}

/// What activating a launcher row does (resolved before borrowing ends).
enum LauncherTarget {
    Launch(String),
    Action(Action),
    Copy(String),
}

struct State {
    komorebi: KStatus,
    /// Last border colours pushed to komorebi (avoid re-sending on every apply).
    komorebi_colours: String,
    komorebi_autostarted: bool,
    drawers: [DrawerState; 4],
    popout: PopoutState,
    active_title: (String, String),
    desktops: Desktops,
    apps: Vec<AppItem>,
    apps_indexed_at: Option<Instant>,
    hits: Vec<Hit>,
    wallpapers: Vec<PathBuf>,
    wallpaper: Option<PathBuf>,
    /// Settings grid page size grows with "Show more".
    wallpaper_limit: usize,
    /// Bumped by every wallpaper change: stale scheme generations are dropped.
    wallpaper_gen: u64,
    wallpaper_preview: Option<WallpaperPreview>,
    /// GitHub wallpaper download in progress, and files done at the last grid refresh.
    wallpaper_dl: Option<Download>,
    wallpaper_dl_shown: usize,
    keep_awake: bool,
    dnd: bool,
    audio: AudioState,
    net: NetSnapshot,
    clock_key: String,
    /// Months away from today shown by the calendars.
    calendar_offset: i32,
    /// Active UI language ("en"/"es"), to refresh Rust-made strings on change.
    lang: String,
    /// Live-preview picker in the launcher (scheme / variant / opacity).
    picker: Option<Picker>,
    /// Wallpaper source colour cache for generating dynamic schemes quickly.
    source_rgb: Option<(PathBuf, material_colors::color::Rgb)>,
    /// Bluetooth devices being (dis)connected: id -> (started, was_connected).
    bt_busy: std::collections::HashMap<String, (Instant, bool)>,
    /// Last place-search results and a generation counter (debounce/staleness).
    places: Vec<crate::services::weather::Place>,
    place_gen: u32,
    /// Last audio endpoint lists pushed to the UI (avoid churn every second).
    audio_lists: (Vec<audio::AudioDevice>, Vec<audio::AudioDevice>),
    fuzzy: Fuzzy,
    /// Clipboard history (memory only), filled by `clip_watcher`.
    clips: ClipHistory,
    /// Performance mode (reduced effects) and why.
    perf: perf::Tracker,
    weather_query: Option<WeatherQuery>,
    settings_visible: bool,
}

/// What a live-preview picker changes and how to undo it.
/// Wallpaper live preview while browsing in the launcher: what to restore on Esc.
struct WallpaperPreview {
    original: Option<PathBuf>,
    scheme: Scheme,
    applied: bool,
}

struct Picker {
    kind: &'static str,
    ids: Vec<String>,
    original_scheme: Scheme,
    original_opacity: (Value, Value, Value),
}

struct Services {
    sysmon: SysMonitor,
    media: Media,
    weather: WeatherService,
    net: Connectivity,
}

pub struct Caelestia {
    weak: Weak<Caelestia>,
    cfg: RefCell<Config>,
    scheme: RefCell<Scheme>,
    screens: RefCell<Vec<Screen>>,
    popout: PopoutWindow,
    dashboard: DashboardWindow,
    launcher: LauncherWindow,
    utilities: UtilitiesWindow,
    session: SessionWindow,
    settings: SettingsWindow,
    st: RefCell<State>,
    svc: RefCell<Option<Services>>,
    hotkeys: RefCell<Option<Hotkeys>>,
    timers: RefCell<Vec<Timer>>,
    thumb_timer: Timer,
    startup_timer: Timer,
    settings_thumb_timer: Timer,
    /// One blur pane per translucent surface (dashboard, launcher, utilities,
    /// session, popout, settings), created lazily.
    blur_panes: RefCell<Vec<Option<crate::platform::blur::BlurPane>>>,
    blur_timer: Timer,
    /// App icons for the bar's workspaces.
    icons: IconCache,
    /// Click-through for WebView2 overlays left behind by hidden apps.
    ghosts: RefCell<crate::platform::ghosts::GhostGuard>,
    watcher: RefCell<Option<Watcher>>,
    clip_watcher: RefCell<Option<crate::platform::clipboard::ClipboardWatcher>>,
}

thread_local! {
    static APP: RefCell<Weak<Caelestia>> = RefCell::new(Weak::new());
}

/// Runs `f` with the shell on the UI thread (used from service callbacks).
fn post(f: impl FnOnce(&Caelestia) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = APP.with(|a| a.borrow().upgrade()) {
            f(&app);
        }
    });
}

impl Caelestia {
    pub fn new(paths: Paths) -> Result<Rc<Self>> {
        let cfg = Config::load(paths.clone())?;
        let scheme = load_or_generate_scheme(&cfg);

        let app = Rc::new_cyclic(|weak| Caelestia {
            weak: weak.clone(),
            cfg: RefCell::new(cfg),
            scheme: RefCell::new(scheme),
            screens: RefCell::new(Vec::new()),
            popout: PopoutWindow::new().expect("popout window"),
            dashboard: DashboardWindow::new().expect("dashboard window"),
            launcher: LauncherWindow::new().expect("launcher window"),
            utilities: UtilitiesWindow::new().expect("utilities window"),
            session: SessionWindow::new().expect("session window"),
            settings: SettingsWindow::new().expect("settings window"),
            st: RefCell::new(State {
                komorebi: KStatus::Stopped,
                komorebi_colours: String::new(),
                komorebi_autostarted: false,
                drawers: Default::default(),
                popout: PopoutState::default(),
                active_title: Default::default(),
                desktops: Desktops::default(),
                apps: Vec::new(),
                apps_indexed_at: None,
                hits: Vec::new(),
                wallpapers: Vec::new(),
                wallpaper: None,
                wallpaper_limit: WALLPAPER_PAGE,
                wallpaper_gen: 0,
                wallpaper_preview: None,
                wallpaper_dl: None,
                wallpaper_dl_shown: 0,
                keep_awake: false,
                dnd: false,
                audio: AudioState::default(),
                net: NetSnapshot::default(),
                clock_key: String::new(),
                calendar_offset: 0,
                lang: String::new(),
                picker: None,
                source_rgb: None,
                bt_busy: Default::default(),
                places: Vec::new(),
                place_gen: 0,
                audio_lists: Default::default(),
                fuzzy: Fuzzy::default(),
                clips: ClipHistory::new(50),
                perf: perf::Tracker::new(2),
                weather_query: None,
                settings_visible: false,
            }),
            svc: RefCell::new(None),
            hotkeys: RefCell::new(None),
            timers: RefCell::new(Vec::new()),
            thumb_timer: Timer::default(),
            startup_timer: Timer::default(),
            settings_thumb_timer: Timer::default(),
            blur_panes: RefCell::new((0..6).map(|_| None).collect()),
            blur_timer: Timer::default(),
            ghosts: RefCell::default(),
            icons: IconCache::new(40, |key, buf| {
                post(move |app| {
                    app.icons.store(key, buf);
                    app.refresh_workspace_icons();
                })
            }),
            watcher: RefCell::new(None),
            clip_watcher: RefCell::new(None),
        });
        APP.with(|a| *a.borrow_mut() = Rc::downgrade(&app));
        app.st.borrow_mut().wallpaper = current_wallpaper_path(&app.cfg.borrow());
        app.wire_callbacks();
        app.apply_config();
        app.set_static_info();
        Ok(app)
    }

    // ------------------------------------------------------------------
    // startup
    // ------------------------------------------------------------------

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
    fn sync_screens(&self) {
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

    fn wire_global(&self, g: ShellGlobal<'_>) {
        self.wire_global_for(g, None);
    }

    /// `screen` is set for per-monitor bars: workspace actions then target
    /// that monitor (komorebi has workspaces per monitor, like Hyprland).
    fn wire_global_for(&self, g: ShellGlobal<'_>, screen: Option<usize>) {
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
    fn active_screen(&self) -> usize {
        let (x, y) = win::cursor_pos();
        self.screens.borrow().iter().position(|s| s.monitor.rect.contains(x, y)).unwrap_or(0)
    }

    fn screen_geom(&self, idx: usize) -> (Rect, f32) {
        let screens = self.screens.borrow();
        screens.get(idx).or(screens.first()).map(|s| (s.area, s.monitor.scale)).unwrap_or_else(|| {
            let m = win::primary_monitor();
            (win::shell_area(&m), m.scale)
        })
    }

    fn primary_scale(&self) -> f32 {
        self.screens.borrow().first().map(|s| s.monitor.scale).unwrap_or(1.0)
    }

    fn set_active_title(&self, title: &str, app: &str) {
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

    fn wire_callbacks(&self) {
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

    fn start_services(&self) {
        let cfg = self.cfg.borrow();
        let sysmon = SysMonitor::spawn(|snap| post(move |app| app.on_sys(snap)));
        sysmon.interval_ms.store(cfg.i64("dashboard.resourceUpdateInterval").max(250) as u64, Ordering::Relaxed);
        let media = Media::spawn(Duration::from_millis(cfg.i64("dashboard.mediaUpdateInterval").clamp(250, 5000) as u64), |snap| post(move |app| app.on_media(snap)));
        let q = weather_query(&cfg);
        self.st.borrow_mut().weather_query = Some(q.clone());
        let weather = WeatherService::spawn(q, |snap| post(move |app| app.on_weather(snap)));
        let net = Connectivity::spawn(|snap| post(move |app| app.on_net(snap)));
        *self.svc.borrow_mut() = Some(Services { sysmon, media, weather, net });
    }

    fn start_timers(&self) {
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

    fn start_watcher(&self) {
        let paths = self.cfg.borrow().paths.clone();
        match Watcher::spawn(paths.shell_json, paths.scheme_json, |c| post(move |app| app.on_file_changed(c))) {
            Ok(w) => *self.watcher.borrow_mut() = Some(w),
            Err(e) => log::warn!("hot reload disabled: {e}"),
        }
    }

    // ------------------------------------------------------------------
    // config + theme
    // ------------------------------------------------------------------

    fn bar_width(&self) -> f32 {
        self.cfg.borrow().f32("bar.sizes.innerWidth").max(24.0) + 8.0
    }

    fn perf_reason(&self) -> Option<PerfReason> {
        self.st.borrow().perf.state()
    }

    /// Animation speed, or (almost) instant while in performance mode.
    fn anim_scale(&self) -> f32 {
        if self.perf_reason().is_some() { 0.0 } else { self.cfg.borrow().f32("appearance.anim.durations.scale") }
    }

    /// Re-evaluates performance mode (every 2 s and on config changes).
    fn refresh_perf(&self) {
        let policy = {
            let cfg = self.cfg.borrow();
            perf::Policy {
                setting: perf::Setting::parse(&cfg.str("windows.performance.mode")),
                on_battery: cfg.bool("windows.performance.onBattery"),
                on_fullscreen: cfg.bool("windows.performance.onFullscreen"),
            }
        };
        // Skip the system queries when the answer can't depend on them.
        let inputs = if policy.setting == perf::Setting::Auto {
            let b = system::battery();
            perf::Inputs { on_battery: b.on_battery, battery_saver: b.saver, fullscreen: policy.on_fullscreen && win::fullscreen_foreground() }
        } else {
            perf::Inputs::default()
        };
        let changed = self.st.borrow_mut().perf.update(perf::decide(policy, inputs));
        if let Some(state) = changed {
            log::info!("performance mode: {}", state.map(PerfReason::id).unwrap_or("off"));
            self.apply_config(); // also re-applies the bar blur
            if state.is_some() {
                self.update_blur(); // hides the panes now; the loop stops itself
            }
        }
    }

    fn apply_config(&self) {
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
            self.push_komorebi_theme();
        }
        self.tick_clock_force();
    }

    fn bind_hotkeys(&self) {
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

    fn set_config(&self, path: &str, json: &str) {
        // Slider values arrive locale-formatted ("0,85" in Spanish): keep them numeric.
        let value: Value = serde_json::from_str(json)
            .ok()
            .or_else(|| parse_num::<f64>(json).and_then(serde_json::Number::from_f64).map(Value::Number))
            .unwrap_or_else(|| Value::String(json.to_owned()));
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
        // Integral settings stay integers in the file (nicer diffs for shared dots).
        let value = match value {
            Value::Number(n) if n.as_f64().is_some_and(|f| f.fract() == 0.0 && is_integral_key(path)) => Value::from(n.as_f64().unwrap() as i64),
            Value::Number(n) => Value::from((n.as_f64().unwrap_or(0.0) * 1000.0).round() / 1000.0),
            v => v,
        };
        let result = {
            let mut cfg = self.cfg.borrow_mut();
            cfg.set(path, value).and_then(|_| cfg.save())
        };
        if let Err(e) = result {
            log::error!("saving config: {e:#}");
        }
        self.apply_config();
        if path.starts_with("windows.performance.") {
            self.refresh_perf();
        }
        if path == "windows.komorebi.enabled" {
            let on = self.cfg.borrow().bool("windows.komorebi.enabled");
            let status = self.st.borrow().komorebi.clone();
            self.st.borrow_mut().desktops = Desktops::default();
            self.st.borrow_mut().komorebi_colours.clear();
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

    fn on_file_changed(&self, which: Changed) {
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
    fn set_scheme(&self, variant: Option<String>, dark: Option<bool>) {
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

    fn set_wallpaper(&self, path: PathBuf) {
        self.apply_wallpaper(path, true);
    }

    /// Shows `path` on the desktop and recolours the shell from it. `persist`
    /// false is the launcher's live preview: nothing is written to disk.
    fn apply_wallpaper(&self, path: PathBuf, persist: bool) {
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
    fn preview_wallpaper(&self, i: usize) {
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
    fn wallpaper_preview_cancel(&self) {
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

    // ------------------------------------------------------------------
    // geometry
    // ------------------------------------------------------------------

    fn layout_screens(&self) {
        let (bar_l, t_l, r_l, persistent, blur) = {
            let cfg = self.cfg.borrow();
            let blur = cfg.bool("appearance.transparency.enabled") && cfg.bool("windows.blur") && self.perf_reason().is_none();
            (self.bar_width(), cfg.f32("border.thickness"), cfg.f32("border.rounding"), cfg.bool("bar.persistent"), blur)
        };
        for s in self.screens.borrow_mut().iter_mut() {
            s.area = win::shell_area(&s.monitor);
            let (area, sc) = (s.area, s.monitor.scale);
            let bar_w = (bar_l * sc).round() as i32;
            let t = (t_l * sc).round() as i32;
            s.border.window().set_position(PhysicalPosition::new(area.left, area.top));
            s.border.window().set_size(slint::PhysicalSize::new(area.width() as u32, area.height() as u32));
            s.bar.window().set_position(PhysicalPosition::new(area.left, area.top));
            s.bar.window().set_size(slint::PhysicalSize::new(bar_w as u32, area.height() as u32));
            if let Some(hwnd) = win::hwnd_of(s.border.window()) {
                win::set_frame_region(hwnd, area.width(), area.height(), bar_w, t, (r_l * sc).round() as i32);
                // DWM blur ignores window regions: never blur the full-screen frame.
                win::set_blur_behind(hwnd, false);
            }
            if let Some(hwnd) = win::hwnd_of(s.bar.window()) {
                win::set_blur_behind(hwnd, blur);
            }
            if persistent {
                if s.appbars.is_empty() {
                    s.appbars = [AbEdge::Left, AbEdge::Top, AbEdge::Right, AbEdge::Bottom].into_iter().filter_map(AppBar::new).collect();
                }
                for b in &s.appbars {
                    b.reserve(&area, if b.edge() == AbEdge::Left { bar_w } else { t });
                }
            } else {
                s.appbars.clear();
            }
        }
    }

    fn style_surface(&self, window: &slint::Window, style: SurfaceStyle) {
        if let Some(hwnd) = win::hwnd_of(window) {
            win::make_surface(hwnd, style);
        }
    }

    fn drawer_window(&self, d: Drawer) -> &slint::Window {
        match d {
            Drawer::Dashboard => self.dashboard.window(),
            Drawer::Launcher => self.launcher.window(),
            Drawer::Utilities => self.utilities.window(),
            Drawer::Session => self.session.window(),
        }
    }

    fn set_drawer_flag(&self, d: Drawer, open: bool) {
        match d {
            Drawer::Dashboard => self.dashboard.set_open(open),
            Drawer::Launcher => self.launcher.set_open(open),
            Drawer::Utilities => self.utilities.set_open(open),
            Drawer::Session => self.session.set_open(open),
        }
    }

    /// Places a drawer against its screen edge, flush with the border.
    fn position_drawer(&self, d: Drawer) -> Rect {
        let window = self.drawer_window(d);
        let size = window.size();
        let (w, h) = (size.width as i32, size.height as i32);
        let screen = self.st.borrow().drawers[d as usize].screen;
        let (area, s) = self.screen_geom(screen);
        let bar_w = (self.bar_width() * s).round() as i32;
        let t = (self.cfg.borrow().f32("border.thickness") * s).round() as i32;
        let inner_left = area.left + bar_w;
        let inner_w = area.width() - bar_w - t;
        let (x, y) = match d {
            Drawer::Dashboard => (inner_left + (inner_w - w) / 2, area.top + t),
            Drawer::Launcher => (inner_left + (inner_w - w) / 2, area.bottom - t - h),
            Drawer::Utilities => (area.right - t - w, area.bottom - t - h),
            Drawer::Session => (area.right - t - w, area.top + (area.height() - h) / 2),
        };
        window.set_position(PhysicalPosition::new(x, y));
        Rect { left: x, top: y, right: x + w, bottom: y + h }
    }

    // ------------------------------------------------------------------
    // drawers
    // ------------------------------------------------------------------

    pub fn toggle(&self, d: Drawer) {
        if self.st.borrow().drawers[d as usize].open {
            self.close(d);
        } else {
            self.open(d, false);
        }
    }

    pub fn open(&self, d: Drawer, by_hover: bool) {
        if d == Drawer::Dashboard && !self.cfg.borrow().bool("dashboard.enabled") {
            return;
        }
        if !by_hover {
            // A deliberate open replaces any other drawer (except a hover-peeked one).
            for other in Drawer::ALL {
                if other != d && self.st.borrow().drawers[other as usize].open {
                    self.close(other);
                }
            }
        }
        let screen = self.active_screen();
        self.st.borrow_mut().drawers[d as usize].screen = screen;
        let window = self.drawer_window(d);
        if let Err(e) = window.show() {
            log::error!("show drawer: {e}");
            return;
        }
        let interactive = matches!(d, Drawer::Launcher | Drawer::Session);
        self.style_surface(window, SurfaceStyle { click_through: false, no_activate: !interactive, topmost: true });
        let rect = self.position_drawer(d);
        // Moving to a monitor with another DPI rescales the window: place it again.
        let weak = self.weak.clone();
        Timer::single_shot(Duration::from_millis(60), move || {
            if let Some(app) = weak.upgrade() {
                if app.st.borrow().drawers[d as usize].open {
                    let r = app.position_drawer(d);
                    app.st.borrow_mut().drawers[d as usize].rect = Some(r);
                }
            }
        });
        {
            let mut st = self.st.borrow_mut();
            let ds = &mut st.drawers[d as usize];
            ds.open = true;
            ds.by_hover = by_hover;
            ds.rect = Some(rect);
            ds.generation += 1;
        }
        match d {
            Drawer::Dashboard => {
                if let Some(svc) = self.svc.borrow().as_ref() {
                    svc.sysmon.active.store(true, Ordering::Relaxed);
                }
            }
            Drawer::Launcher => {
                self.launcher.set_picker_mode("".into());
                self.launcher.set_wallpaper_mode(false);
                self.launcher.invoke_reset();
                if self.st.borrow().apps_indexed_at.is_none_or(|t| t.elapsed() > Duration::from_secs(300)) {
                    self.reindex_apps();
                }
                self.search("");
            }
            Drawer::Session => self.session.set_focused(1),
            Drawer::Utilities => self.refresh_recordings(),
        }
        self.set_drawer_flag(d, true);
        self.ensure_blur_tracking();
        if interactive {
            self.focus_drawer(d);
            // The native window may only become activatable a frame later
            // (first show / DPI move): retry briefly until it has focus.
            for delay in [40u64, 120, 300] {
                let weak = self.weak.clone();
                Timer::single_shot(Duration::from_millis(delay), move || {
                    if let Some(app) = weak.upgrade() {
                        if app.st.borrow().drawers[d as usize].open {
                            app.focus_drawer(d);
                        }
                    }
                });
            }
        }
    }

    /// Gives a keyboard-driven drawer OS focus and puts the caret where the
    /// user types (launcher search / session buttons).
    fn focus_drawer(&self, d: Drawer) {
        let Some(hwnd) = win::hwnd_of(self.drawer_window(d)) else { return };
        if win::foreground_window() != hwnd {
            win::focus_window(hwnd);
        }
        match d {
            Drawer::Launcher => self.launcher.invoke_focus_search(),
            Drawer::Session => self.session.invoke_focus_keys(),
            _ => {}
        }
    }

    pub fn close(&self, d: Drawer) {
        let generation = {
            let mut st = self.st.borrow_mut();
            let ds = &mut st.drawers[d as usize];
            if !ds.open {
                return;
            }
            ds.open = false;
            ds.by_hover = false;
            ds.generation += 1;
            ds.generation
        };
        self.set_drawer_flag(d, false);
        if d == Drawer::Launcher {
            self.picker_cancel();
            self.wallpaper_preview_cancel();
        }
        if d == Drawer::Dashboard {
            if let Some(svc) = self.svc.borrow().as_ref() {
                svc.sysmon.active.store(false, Ordering::Relaxed);
            }
        }
        // Hide once the close animation has finished, unless reopened meanwhile.
        let weak = self.weak.clone();
        let anim = (300.0 * self.anim_scale().max(0.1)) as u64;
        Timer::single_shot(Duration::from_millis(anim + 60), move || {
            if let Some(app) = weak.upgrade() {
                let ds = app.st.borrow().drawers[d as usize];
                if ds.generation == generation && !ds.open {
                    let _ = app.drawer_window(d).hide();
                }
            }
        });
    }

    fn close_all(&self) {
        for d in Drawer::ALL {
            self.close(d);
        }
    }

    fn toggle_settings(&self, show: Option<bool>) {
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

    fn apply_settings_backdrop(&self) {
        if !self.st.borrow().settings_visible {
            return;
        }
        if let Some(hwnd) = win::hwnd_of(self.settings.window()) {
            let cfg = self.cfg.borrow();
            let _ = cfg;
            win::set_backdrop(hwnd, false, self.scheme.borrow().is_dark());
        }
    }

    /// Hover-to-open for the dashboard (top edge) and utilities (bottom-right
    /// corner) on whichever screen the pointer is, plus popout dismissal.
    fn poll_hover(&self) {
        let (x, y) = win::cursor_pos();
        self.poll_popout(x, y);
        let idx = self.active_screen();
        let (area, s) = self.screen_geom(idx);
        let drawers = self.st.borrow().drawers;
        let cfg = self.cfg.borrow();
        let t = (cfg.f32("border.thickness") * s).round() as i32;
        let edge = t.max(2) + 2;
        // A full-screen game must never get a drawer from the mouse touching an edge.
        let hover_ok = self.perf_reason() != Some(PerfReason::Fullscreen);
        let dash_on = hover_ok && cfg.bool("dashboard.enabled") && cfg.bool("dashboard.showOnHover");
        let util_on = hover_ok && cfg.bool("utilities.enabled") && cfg.bool("utilities.showOnHover");
        drop(cfg);

        let check = |d: Drawer, zone: Rect| {
            let ds = drawers[d as usize];
            if !ds.open && area.contains(x, y) && zone.contains(x, y) {
                self.open(d, true);
            } else if ds.open && ds.by_hover {
                let inside = ds.rect.map(|r| r.inflate((8.0 * s) as i32).contains(x, y)).unwrap_or(false);
                if !inside && !(ds.screen == idx && zone.contains(x, y)) {
                    self.close(d);
                }
            }
        };
        if dash_on {
            let w = drawers[0].rect.map(|r| r.width()).unwrap_or((940.0 * s) as i32);
            let cx = area.left + (area.width() + (self.bar_width() * s) as i32 - t) / 2;
            check(Drawer::Dashboard, Rect { left: cx - w / 2, right: cx + w / 2, top: area.top, bottom: area.top + edge });
        }
        if util_on {
            let w = drawers[2].rect.map(|r| r.width()).unwrap_or((330.0 * s) as i32);
            check(Drawer::Utilities, Rect { left: area.right - w, right: area.right, top: area.bottom - edge, bottom: area.bottom });
        }
    }

    // ------------------------------------------------------------------
    // bar popouts
    // ------------------------------------------------------------------

    /// `arg` is `<kind>:<logical y of the item centre within the bar>`.
    fn show_popout(&self, arg: &str) {
        let Some((kind, y)) = arg.split_once(':') else { return };
        let y: f32 = parse_num(y).unwrap_or(0.0);
        let (area, sc) = self.screen_geom(self.active_screen());
        let was_open = self.st.borrow().popout.open;
        self.popout.set_kind(kind.into());
        if self.popout.show().is_err() {
            return;
        }
        if let Some(h) = win::hwnd_of(self.popout.window()) {
            win::make_surface(h, SurfaceStyle { click_through: false, no_activate: true, topmost: true });
        }
        // The window has a fixed (max) size; the panel inside morphs and is
        // vertically centred, so we centre the window on the hovered item.
        let (w, h) = ((self.popout.get_want_width() * sc).round() as i32, (self.popout.get_want_height() * sc).round() as i32);
        let half_panel = (self.popout.invoke_panel_height_for(kind.into()) * sc / 2.0).round() as i32;
        let t = (self.cfg.borrow().f32("border.thickness") * sc).round() as i32;
        let x = area.left + (self.bar_width() * sc).round() as i32;
        let cy = (area.top + (y * sc).round() as i32).clamp(area.top + t + half_panel, (area.bottom - t - half_panel).max(area.top + t + half_panel));
        let top = cy - h / 2;
        self.popout.window().set_size(slint::PhysicalSize::new(w as u32, h as u32));
        self.popout.window().set_position(PhysicalPosition::new(x, top));
        {
            let mut st = self.st.borrow_mut();
            let p = &mut st.popout;
            p.open = true;
            p.kind = kind.to_owned();
            p.item_hovered = true;
            p.rect = Some(Rect { left: x, top: cy - half_panel, right: x + w, bottom: cy + half_panel });
            p.generation += 1;
        }
        if !was_open {
            self.popout.set_open(true);
        }
        self.ensure_blur_tracking();
    }

    /// Runs the blur-follow loop while any translucent surface is on screen.
    fn ensure_blur_tracking(&self) {
        if self.blur_timer.running() {
            return;
        }
        let weak = self.weak.clone();
        self.blur_timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
            if let Some(app) = weak.upgrade() {
                app.update_blur();
            }
        });
    }

    fn update_blur(&self) {
        use crate::platform::blur::{Attached, BlurPane};
        let (enabled, border_r, settings_r) = {
            let cfg = self.cfg.borrow();
            (
                cfg.bool("appearance.transparency.enabled") && cfg.bool("windows.blur") && self.perf_reason().is_none(),
                cfg.f32("border.rounding"),
                25.0 * cfg.f32("appearance.rounding.scale"),
            )
        };
        let a = |left, top, right, bottom| Attached { left, top, right, bottom };
        let geo = |x: f32, y: f32, w: f32, h: f32| (x, y, w, h);
        let surfaces: [(&slint::Window, (f32, f32, f32, f32), Attached, f32); 6] = [
            (self.dashboard.window(), geo(self.dashboard.get_blur_x(), self.dashboard.get_blur_y(), self.dashboard.get_blur_w(), self.dashboard.get_blur_h()), a(false, true, false, false), border_r),
            (self.launcher.window(), geo(self.launcher.get_blur_x(), self.launcher.get_blur_y(), self.launcher.get_blur_w(), self.launcher.get_blur_h()), a(false, false, false, true), border_r),
            (self.utilities.window(), geo(self.utilities.get_blur_x(), self.utilities.get_blur_y(), self.utilities.get_blur_w(), self.utilities.get_blur_h()), a(false, false, true, true), border_r),
            (self.session.window(), geo(self.session.get_blur_x(), self.session.get_blur_y(), self.session.get_blur_w(), self.session.get_blur_h()), a(false, false, true, false), border_r),
            (self.popout.window(), geo(self.popout.get_blur_x(), self.popout.get_blur_y(), self.popout.get_blur_w(), self.popout.get_blur_h()), a(true, false, false, false), border_r),
            (self.settings.window(), geo(0.0, 0.0, 885.0, 625.0), a(false, false, false, false), settings_r),
        ];
        let mut any = false;
        let mut panes = self.blur_panes.borrow_mut();
        for (i, (window, (x, y, w, h), attached, radius)) in surfaces.into_iter().enumerate() {
            let hwnd = win::hwnd_of(window).filter(|h| unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(*h).as_bool() });
            let settings_hidden = i == 5 && !self.st.borrow().settings_visible;
            let Some(hwnd) = hwnd.filter(|_| enabled && !settings_hidden) else {
                if let Some(p) = panes[i].as_ref() {
                    p.hide();
                }
                continue;
            };
            any = true;
            if panes[i].is_none() {
                panes[i] = BlurPane::new();
            }
            let Some(pane) = panes[i].as_ref() else { continue };
            let sf = window.scale_factor();
            let pos = window.position();
            let rect = Rect {
                left: pos.x + (x * sf).round() as i32,
                top: pos.y + (y * sf).round() as i32,
                right: pos.x + ((x + w) * sf).round() as i32,
                bottom: pos.y + ((y + h) * sf).round() as i32,
            };
            pane.follow(hwnd, rect, attached, radius * sf, sf);
        }
        if !any {
            self.blur_timer.stop();
        }
    }

    fn close_popout(&self) {
        let generation = {
            let mut st = self.st.borrow_mut();
            if !st.popout.open {
                return;
            }
            st.popout.open = false;
            st.popout.generation += 1;
            st.popout.generation
        };
        self.popout.set_open(false);
        let weak = self.weak.clone();
        Timer::single_shot(Duration::from_millis(320), move || {
            if let Some(app) = weak.upgrade() {
                let p = app.st.borrow().popout.clone();
                if !p.open && p.generation == generation {
                    let _ = app.popout.hide();
                }
            }
        });
    }

    fn poll_popout(&self, x: i32, y: i32) {
        let p = self.st.borrow().popout.clone();
        if !p.open || p.item_hovered {
            return;
        }
        let inside = p.rect.map(|r| r.inflate(8).contains(x, y)).unwrap_or(false);
        if !inside {
            self.close_popout();
        }
    }

    fn on_foreground(&self, hwnd: windows::Win32::Foundation::HWND) {
        // Another app took focus: dismiss click-opened drawers (like clicking outside a popup).
        if let Some(info) = desktops::window_info(hwnd) {
            for d in Drawer::ALL {
                let ds = self.st.borrow().drawers[d as usize];
                if ds.open && !ds.by_hover {
                    self.close(d);
                }
            }
            self.set_active_title(&info.title, &info.app);
        }
        self.refresh_workspaces();
    }

    fn komorebi_active(&self) -> bool {
        self.cfg.borrow().bool("windows.komorebi.enabled") && matches!(self.st.borrow().komorebi, KStatus::Running(_))
    }

    fn on_komorebi(&self, status: KStatus) {
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

    fn komorebi_monitor_for(&self, screen: usize, state: &KState) -> Option<usize> {
        let screens = self.screens.borrow();
        let s = screens.get(screen)?;
        state
            .monitors
            .iter()
            .position(|m| m.left == s.monitor.rect.left && m.top == s.monitor.rect.top)
            .or_else(|| (screen < state.monitors.len()).then_some(screen))
    }

    fn apply_komorebi_workspaces(&self, state: &KState) {
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
    fn workspace_model(&self, entries: &[(String, Vec<isize>)]) -> ModelRc<Workspace> {
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
    fn refresh_workspace_icons(&self) {
        if let KStatus::Running(state) = self.st.borrow().komorebi.clone() {
            if self.komorebi_active() {
                return self.apply_komorebi_workspaces(&state);
            }
        }
        self.st.borrow_mut().desktops = Desktops::default();
        self.refresh_workspaces();
    }

    fn komorebi_monitor_for_rect(&self, mon: &Monitor, state: &KState, fallback: usize) -> Option<usize> {
        state
            .monitors
            .iter()
            .position(|m| m.left == mon.rect.left && m.top == mon.rect.top)
            .or_else(|| (fallback < state.monitors.len()).then_some(fallback))
    }

    /// Workspace click/scroll on a given screen's bar.
    fn komorebi_workspace(&self, screen: usize, id: &str, arg: &str) {
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
    fn push_komorebi_theme(&self) {
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

    fn on_ipc(&self, cmd: &str) {
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

    fn on_hotkey(&self, target: &str) {
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

    // ------------------------------------------------------------------
    // actions from the UI
    // ------------------------------------------------------------------

    fn action(&self, id: &str, arg: &str) {
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
            }
            "dots-export" => self.export_dots(),
            "dots-import" => self.import_dots(),
            "open-wallpapers" => {
                self.toggle_settings(Some(false));
                self.open_wallpaper_picker();
            }
            other => log::debug!("unhandled action {other}({arg})"),
        }
    }

    fn radio(&self, cmd: RadioCmd) {
        if let Some(svc) = self.svc.borrow().as_ref() {
            svc.net.send(cmd);
        }
    }

    // ------------------------------------------------------------------
    // launcher
    // ------------------------------------------------------------------

    fn reindex_apps(&self) {
        let hidden = self.cfg.borrow().str_list("launcher.hiddenApps");
        let px = (32.0 * self.primary_scale()).round() as i32;
        spawn_index(hidden, px.max(32), |records| post(move |app| app.on_apps(records)));
    }

    fn on_apps(&self, records: Vec<AppRecord>) {
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

    fn launcher_prefixes(&self) -> Prefixes {
        let cfg = self.cfg.borrow();
        Prefixes { actions: cfg.str("launcher.actionPrefix"), calc: cfg.str("launcher.calcPrefix"), emoji: cfg.str("launcher.emojiPrefix"), clipboard: cfg.str("launcher.clipboardPrefix") }
    }

    fn search(&self, query: &str) {
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
    fn open_launcher_mode(&self, mode: LauncherMode) {
        if !self.st.borrow().drawers[Drawer::Launcher as usize].open {
            self.open(Drawer::Launcher, false);
        }
        let prefix = self.launcher_prefixes().get(mode).to_owned();
        self.launcher.set_query(prefix.clone().into());
        self.search(&prefix);
        self.focus_drawer(Drawer::Launcher);
    }

    fn activate_hit(&self, index: usize) {
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
    fn copy_and_close(&self, text: &str) {
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
    fn remove_launcher_entry(&self, index: usize) {
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
    fn sync_clipboard_watcher(&self) {
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

    fn on_clipboard_text(&self, text: String) {
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

    fn run_launcher_action(&self, a: Action) {
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

    // ------------------------------------------------------------------
    // live-preview pickers (launcher carousel)
    // ------------------------------------------------------------------

    /// Seed colour of the current wallpaper (cached; decoding is ~100 ms).
    fn wallpaper_source(&self) -> material_colors::color::Rgb {
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

    fn open_picker(&self, kind: &'static str) {
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
    fn picker_preview(&self, i: usize) {
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
    fn picker_confirm(&self, i: usize) {
        self.picker_preview(i);
        let Some(picker) = self.st.borrow_mut().picker.take() else { return };
        let result = if picker.kind == "opacity" { self.cfg.borrow_mut().save() } else { self.scheme.borrow().save(&self.cfg.borrow().paths.scheme_json) };
        if let Err(e) = result {
            log::error!("saving picker choice: {e:#}");
        }
        self.st.borrow_mut().komorebi_colours.clear();
        self.apply_config(); // also syncs komorebi borders now that the picker is done
        self.close(Drawer::Launcher);
    }

    /// Esc / closing the launcher: restore what was there before previewing.
    fn picker_cancel(&self) {
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

    fn open_wallpaper_picker(&self) {
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

    // ------------------------------------------------------------------
    // periodic refreshers
    // ------------------------------------------------------------------

    fn tick_clock_force(&self) {
        self.st.borrow_mut().clock_key.clear();
        self.tick_clock();
    }

    fn tick_clock(&self) {
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
        let g = self.dashboard.global::<ShellGlobal>();
        let mut user = g.get_user();
        user.uptime = crate::i18n::uptime(system::uptime_secs()).into();
        g.set_user(user);
    }

    fn refresh_status(&self) {
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
        self.push_audio_devices();
    }

    fn refresh_workspaces(&self) {
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

    fn refresh_recordings(&self) {
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

    // ------------------------------------------------------------------
    // service updates
    // ------------------------------------------------------------------

    fn on_sys(&self, s: SysSnapshot) {
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

    fn on_media(&self, m: MediaSnapshot) {
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

    fn on_weather(&self, w: Option<WeatherSnapshot>) {
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
    fn place_search(&self, text: &str) {
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
    fn set_weather_place(&self, place: Option<crate::services::weather::Place>) {
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
    fn refresh_dnd(&self) {
        if let Some(on) = crate::platform::focus::dnd_enabled() {
            self.st.borrow_mut().dnd = on;
            for g in [self.utilities.global::<ShellGlobal>(), self.popout.global::<ShellGlobal>()] {
                if g.get_dnd() != on {
                    g.set_dnd(on);
                }
            }
        }
    }

    fn on_net(&self, n: NetSnapshot) {
        self.st.borrow_mut().net = n;
        self.refresh_status();
        self.push_bt_devices();
    }

    /// Paired Bluetooth devices -> UI, with a "busy" flag while a
    /// (dis)connection is in flight (until the state flips or 10 s pass).
    fn push_bt_devices(&self) {
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

    /// Audio endpoint lists -> UI (only when they change).
    fn push_audio_devices(&self) {
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

    // ------------------------------------------------------------------
    // settings panel
    // ------------------------------------------------------------------

    fn set_static_info(&self) {
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

    fn update_wallpaper_preview(&self) {
        let Some(path) = self.st.borrow().wallpaper.clone() else { return };
        let px = (320.0 * self.primary_scale()) as i32;
        // Shell thumbnails are cached by Explorer and decode quickly.
        let buf = path_thumbnail(&path, px, false);
        self.settings.set_has_wallpaper_preview(buf.is_some());
        if let Some(buf) = buf {
            self.settings.set_wallpaper_preview(Image::from_rgba8_premultiplied(buf));
        }
    }

    fn random_wallpaper(&self) {
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

    /// `dynamic` or `<name>:<flavour>` (e.g. `catppuccin:mocha`).
    fn apply_scheme_id(&self, id: &str) {
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
    fn opacity_preset(&self, id: &str) {
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

    fn refresh_scheme_cards(&self) {
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

    /// Wallpaper grid in Settings, one page at a time (a big folder would
    /// otherwise hold hundreds of MB of thumbnails); thumbnails stream in off-thread.
    fn refresh_settings_wallpapers(&self) {
        let walls = list_wallpapers(&self.cfg.borrow());
        let limit = self.st.borrow().wallpaper_limit;
        self.settings.set_wallpaper_dir(wallpaper_dir(&self.cfg.borrow()).display().to_string().into());
        self.settings.set_wallpapers_total(walls.len() as i32);
        let model = Rc::new(VecModel::from(walls.iter().take(limit).map(|p| self.wallpaper_entry(p)).collect::<Vec<_>>()));
        self.settings.set_wallpapers(ModelRc::from(model.clone()));
        self.stream_settings_thumbs(model);
    }

    /// "Show more": appends the next page without reloading loaded thumbnails.
    fn more_settings_wallpapers(&self) {
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

    fn wallpaper_entry(&self, p: &Path) -> WallpaperEntry {
        let current = self.st.borrow().wallpaper.as_deref() == Some(p);
        WallpaperEntry {
            path: p.display().to_string().into(),
            name: p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default().into(),
            thumb: Image::default(),
            current,
        }
    }

    fn stream_settings_thumbs(&self, model: Rc<VecModel<WallpaperEntry>>) {
        self.stream_settings_thumbs_model(ModelRc::from(model));
    }

    /// Loads thumbnails for the grid rows that don't have one yet.
    fn stream_settings_thumbs_model(&self, model: ModelRc<WallpaperEntry>) {
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
    fn start_wallpaper_download(&self, url: &str) {
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

    fn on_wallpaper_download(&self, p: DlProgress) {
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
    fn mark_current_wallpaper(&self) {
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

    fn settings_search(&self, q: &str) {
        const KEYWORDS: [&str; 10] = [
            "wallpaper style fonts colours colors scheme transparency dark light theme rounding fondo estilo fuentes colores esquema transparencia oscuro claro tema opacidad",
            "network wifi wi-fi ethernet internet vpn red redes",
            "connected devices bluetooth pairing printers dispositivos emparejar impresoras auriculares",
            "audio volume sound microphone mute speakers volumen sonido microfono silenciar altavoces salida entrada",
            "updates system windows update winget actualizaciones sistema",
            "plugins extensions extensiones complementos",
            "panels dashboard taskbar bar launcher sidebar border workspaces clock paneles panel barra lanzador marco espacios reloj komorebi mosaico",
            "services poll intervals lyrics weather location servicios clima ubicacion ciudad",
            "language region locale weather location units fahrenheit clock 12-hour idioma region ubicacion unidades reloj espanol",
            "about system information credits version dots export import config acerca sistema informacion version exportar importar configuracion",
        ];
        let q = q.trim().to_lowercase();
        let matches: Vec<bool> = if q.is_empty() { vec![] } else { KEYWORDS.iter().map(|k| q.split_whitespace().all(|w| k.contains(w))).collect() };
        self.settings.set_nav_match(ModelRc::new(VecModel::from(matches)));
    }

    fn export_dots(&self) {
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

    fn import_dots(&self) {
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

impl Caelestia {
    /// Releases everything visible to the rest of the system (reserved screen
    /// space, keep-awake). Background threads are left to process exit.
    pub fn shutdown(&self) {
        system::keep_awake(false);
        self.ghosts.borrow_mut().restore_all();
        for s in self.screens.borrow_mut().iter_mut() {
            s.appbars.clear();
            let _ = s.bar.hide();
            let _ = s.border.hide();
        }
    }
}

impl Drop for Caelestia {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ----------------------------------------------------------------------
// helpers
// ----------------------------------------------------------------------

/// Switches the bar's active workspace, telling it the direction first so
/// the active trail stretches the right way.
fn set_active_workspace(g: &ShellGlobal<'_>, active: i32) {
    let previous = g.get_active_workspace();
    if previous != active {
        g.set_workspace_direction(workspace_direction(previous, active));
        g.set_active_workspace(active);
    }
}

fn workspace_direction(from: i32, to: i32) -> i32 {
    if to < from { -1 } else { 1 }
}

fn is_integral_key(path: &str) -> bool {
    matches!(path, "border.thickness" | "border.rounding" | "bar.workspaces.shown" | "launcher.maxShown" | "bar.sizes.innerWidth") || path.starts_with("windows.komorebi.")
}

/// Windows locale, e.g. "es-CR".
fn system_locale() -> String {
    let mut buf = [0u16; 85];
    let n = unsafe { windows::Win32::Globalization::GetUserDefaultLocaleName(&mut buf) };
    String::from_utf16_lossy(&buf[..(n.max(1) as usize - 1)])
}

/// Two-letter UI language of Windows (for localised place names).
fn system_lang() -> String {
    system_locale().split('-').next().filter(|s| s.len() == 2).unwrap_or("en").to_lowercase()
}

/// Two-letter region of Windows ("CR"), used to rank local places first.
fn system_region() -> String {
    system_locale().rsplit('-').next().filter(|s| s.len() == 2).unwrap_or_default().to_uppercase()
}

fn weather_query(cfg: &Config) -> WeatherQuery {
    WeatherQuery { lang: crate::i18n::lang(), location: cfg.str("services.weatherLocation"), name: cfg.str("services.weatherLocationName"), fahrenheit: cfg.bool("services.useFahrenheit"), twelve_hour: cfg.bool("services.useTwelveHourClock") }
}

fn recordings_dir(cfg: &Config) -> PathBuf {
    let p = cfg.path("paths.recordingsDir");
    if p.as_os_str().is_empty() { dirs::video_dir().unwrap_or_default() } else { p }
}

fn current_wallpaper_path(cfg: &Config) -> Option<PathBuf> {
    std::fs::read_to_string(cfg.paths.state_dir.join("wallpaper").join("path.txt"))
        .ok()
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| p.exists())
        .or_else(|| system::current_wallpaper().map(PathBuf::from).filter(|p| p.exists()))
}

fn load_or_generate_scheme(cfg: &Config) -> Scheme {
    if let Ok(s) = Scheme::load(&cfg.paths.scheme_json) {
        return s;
    }
    let source = current_wallpaper_path(cfg).and_then(|p| source_from_image(&p).ok()).unwrap_or(material_colors::color::Rgb::from_u32(0x4a6fa5));
    let scheme = Scheme::generate(source, "tonalspot", true);
    if let Err(e) = scheme.save(&cfg.paths.scheme_json) {
        log::warn!("could not write scheme.json: {e:#}");
    }
    scheme
}

/// Configured wallpaper folder, or Windows' built-in wallpapers when it's missing.
fn wallpaper_dir(cfg: &Config) -> PathBuf {
    let dir = cfg.path("paths.wallpaperDir");
    if dir.is_dir() {
        return dir;
    }
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    PathBuf::from(windir).join("Web").join("Wallpaper")
}

/// Settings grid page size.
const WALLPAPER_PAGE: usize = 60;

/// "12 MB" / "1.4 GB" ("1,4 GB" in Spanish).
fn size_label(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    let s = if mb >= 1024.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{mb:.0} MB") };
    if crate::i18n::is_es() { s.replace('.', ",") } else { s }
}

fn list_wallpapers(cfg: &Config) -> Vec<PathBuf> {
    let root = wallpaper_dir(cfg);
    let max = 10_000;
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if matches!(p.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("jpg" | "jpeg" | "png" | "webp" | "bmp")) {
                out.push(p);
                if out.len() >= max {
                    break;
                }
            }
        }
    }
    out.sort();
    out
}

fn average_luminance(path: &Path) -> Option<f32> {
    let img = image::open(path).ok()?.thumbnail(32, 32).into_rgb8();
    let n = (img.width() * img.height()) as f32;
    Some(img.pixels().map(|p| (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0).sum::<f32>() / n)
}

fn add_months(d: NaiveDate, months: i32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + months;
    NaiveDate::from_ymd_opt(total.div_euclid(12), total.rem_euclid(12) as u32 + 1, 1).unwrap_or(d)
}

/// 6×7 grid for the month containing `month`, highlighting `today`.
/// Parses a number Slint formatted as text: float-to-string follows the UI
/// locale, so Spanish sends "0,45".
fn parse_num<T: std::str::FromStr>(s: &str) -> Option<T> {
    s.trim().replace(',', ".").parse().ok()
}

fn calendar(month: NaiveDate, today: NaiveDate) -> (String, Vec<CalendarDay>) {
    let first = month.with_day(1).unwrap_or(month);
    let offset = first.weekday().num_days_from_monday() as i64;
    let start = first - chrono::Duration::days(offset);
    let days = (0..42)
        .map(|i| {
            let d = start + chrono::Duration::days(i);
            CalendarDay { day: d.day() as i32, current_month: d.month() == first.month(), today: d == today }
        })
        .collect();
    (crate::i18n::capitalize(&first.format_localized("%B %Y", crate::i18n::chrono_locale()).to_string()), days)
}

/// Material 3 expressive "9-sided cookie" outline in a 100x100 viewbox.
fn cookie_path() -> String {
    let (n, lobes) = (180, 9.0f64);
    let mut s = String::new();
    for i in 0..=n {
        let a = i as f64 / n as f64 * std::f64::consts::TAU;
        let r = 46.0 * (0.9 + 0.1 * (lobes * a).cos());
        let (x, y) = (50.0 + r * a.cos(), 50.0 + r * a.sin());
        s.push_str(&format!("{}{x:.2} {y:.2}", if i == 0 { "M " } else { " L " }));
    }
    s.push_str(" Z");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_direction_follows_the_switch() {
        assert_eq!(workspace_direction(1, 3), 1);
        assert_eq!(workspace_direction(4, 2), -1);
        assert_eq!(workspace_direction(2, 2), 1);
    }

    #[test]
    fn parses_locale_numbers() {
        // Slint formats floats with the UI locale: Spanish sends "0,45".
        assert_eq!(parse_num::<f64>("0,45"), Some(0.45));
        assert_eq!(parse_num::<f64>(" 0.45 "), Some(0.45));
        assert_eq!(parse_num::<f32>("12"), Some(12.0));
        assert_eq!(parse_num::<f64>("abc"), None);
    }

    #[test]
    fn months_wrap_across_years() {
        let d = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        assert_eq!(add_months(d, -1), NaiveDate::from_ymd_opt(2025, 12, 1).unwrap());
        assert_eq!(add_months(d, 12), NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
        assert_eq!(add_months(d, 0), NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
    }

    #[test]
    fn integral_keys_stay_integers() {
        assert!(is_integral_key("border.thickness"));
        assert!(is_integral_key("windows.komorebi.borderWidth"));
        assert!(!is_integral_key("appearance.transparency.base"));
    }
}
