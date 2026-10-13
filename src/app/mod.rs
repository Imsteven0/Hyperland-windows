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
use slint::{ComponentHandle, Image, ModelRc, PhysicalPosition, Rgba8Pixel, SharedPixelBuffer, SharedString, Timer, TimerMode, VecModel};

use crate::{
    AppEntry, AudioDevice as UiAudioDevice, NotifGroup, NotifItem, PickerEntry, PluginWidget, SidebarWindow, Place, BarWindow, BtDevice as UiBtDevice, BorderWindow, CalendarDay, DashboardWindow, ForecastDay, InfoRow, LauncherWindow, MediaInfo, PopoutWindow, Recording, SessionWindow,
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
        dots_repo,
        icons::IconCache,
        komorebi::{self, KState, KStatus},
        media::{Media, MediaCommand, MediaSnapshot, fmt_time},
        notifications::{self, AccessState, NotifCommand, NotifEvent, Notifications},
        perf::{self, Reason as PerfReason},
        plugins::{self, Plugin, Runner as PluginRunner},
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
        { let $w = &$app.sidebar; $body; }
        { let $w = &$app.settings; $body; }
    }};
}

// One file per area; each adds methods to `Caelestia` (children see its private fields).
mod actions;
mod config;
mod dots;
mod drawers;
mod helpers;
mod launcher;
mod notif_center;
mod performance;
mod pickers;
mod plugin_widgets;
mod popouts;
mod settings;
mod startup;
mod status;
mod wallpaper;
mod workspaces;

use helpers::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drawer {
    Dashboard = 0,
    Launcher = 1,
    Utilities = 2,
    Session = 3,
    /// Notification center (Caelestia's sidebar), top-right corner.
    Sidebar = 4,
}

impl Drawer {
    const ALL: [Drawer; 5] = [Drawer::Dashboard, Drawer::Launcher, Drawer::Utilities, Drawer::Session, Drawer::Sidebar];
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "dashboard" => Self::Dashboard,
            "launcher" => Self::Launcher,
            "utilities" => Self::Utilities,
            "session" => Self::Session,
            "sidebar" | "notifications" => Self::Sidebar,
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

#[derive(Default)]
struct PopoutMotion {
    x: i32,
    y: f32,
    vel: f32,
    target: f32,
    last: Option<Instant>,
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
    drawers: [DrawerState; 5],
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
    /// When the lists were last read (dragging the volume refreshes the
    /// status many times a second; the lists don't need to follow).
    audio_lists_at: Option<Instant>,
    fuzzy: Fuzzy,
    /// Clipboard history (memory only), filled by `clip_watcher`.
    clips: ClipHistory,
    /// Enabled plugins (runner indices), their last results, and every
    /// plugin folder found (for Settings, including disabled/broken ones).
    plugins: Vec<Plugin>,
    plugin_results: Vec<Option<Result<plugins::Output, String>>>,
    plugin_listing: Vec<PluginListing>,
    plugin_gen: u64,
    /// Newer release found by the update check.
    update: Option<crate::services::updates::Release>,
    /// Notification center: current list, app logos, groups shown expanded.
    notifs: Vec<notifications::Notif>,
    notif_icons: std::collections::HashMap<String, Image>,
    notif_expanded: std::collections::HashSet<String>,
    /// Dots fetched from GitHub, waiting for "Apply"; the generation drops
    /// previews that arrive after a newer request.
    dots_preview: Option<dots_repo::Preview>,
    dots_gen: u32,
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
    notifications: Notifications,
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
    sidebar: SidebarWindow,
    settings: SettingsWindow,
    st: RefCell<State>,
    svc: RefCell<Option<Services>>,
    hotkeys: RefCell<Option<Hotkeys>>,
    timers: RefCell<Vec<Timer>>,
    thumb_timer: Timer,
    /// Coalesces workspace rebuilds when app icons finish loading.
    icon_refresh: Timer,
    startup_timer: Timer,
    settings_thumb_timer: Timer,
    /// One blur pane per translucent surface (dashboard, launcher, utilities,
    /// session, popout, settings, sidebar), created lazily.
    blur_panes: RefCell<Vec<Option<crate::platform::blur::BlurPane>>>,
    blur_timer: Timer,
    /// The bar popout gliding between items (physical px); `popout_timer`
    /// steps it only while it moves.
    popout_motion: RefCell<PopoutMotion>,
    popout_timer: Timer,
    /// App icons for the bar's workspaces.
    icons: IconCache,
    /// Click-through for WebView2 overlays left behind by hidden apps.
    ghosts: RefCell<crate::platform::ghosts::GhostGuard>,
    watcher: RefCell<Option<Watcher>>,
    clip_watcher: RefCell<Option<crate::platform::clipboard::ClipboardWatcher>>,
    plugin_runner: RefCell<Option<PluginRunner>>,
    updates: RefCell<Option<crate::services::updates::UpdateChecker>>,
}

/// A plugin folder as Settings shows it.
struct PluginListing {
    id: String,
    name: String,
    icon: String,
    enabled: bool,
    /// Broken manifest.
    error: String,
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
            sidebar: SidebarWindow::new().expect("sidebar window"),
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
                audio_lists_at: None,
                fuzzy: Fuzzy::default(),
                clips: ClipHistory::new(50),
                perf: perf::Tracker::new(2),
                plugins: Vec::new(),
                plugin_results: Vec::new(),
                plugin_listing: Vec::new(),
                plugin_gen: 0,
                update: None,
                notifs: Vec::new(),
                notif_icons: Default::default(),
                notif_expanded: Default::default(),
                dots_preview: None,
                dots_gen: 0,
                weather_query: None,
                settings_visible: false,
            }),
            svc: RefCell::new(None),
            hotkeys: RefCell::new(None),
            timers: RefCell::new(Vec::new()),
            thumb_timer: Timer::default(),
            icon_refresh: Timer::default(),
            startup_timer: Timer::default(),
            settings_thumb_timer: Timer::default(),
            blur_panes: RefCell::new((0..7).map(|_| None).collect()),
            blur_timer: Timer::default(),
            popout_motion: RefCell::new(PopoutMotion::default()),
            popout_timer: Timer::default(),
            ghosts: RefCell::new(crate::platform::ghosts::GhostGuard::with_state(paths.state_dir.join("click-through.json"))),
            icons: IconCache::new(40, |key, buf| {
                post(move |app| {
                    app.icons.store(key, buf);
                    // Icons arrive in bursts (one per app at startup): rebuild once.
                    if !app.icon_refresh.running() {
                        let weak = app.weak.clone();
                        app.icon_refresh.start(TimerMode::SingleShot, Duration::from_millis(60), move || {
                            if let Some(app) = weak.upgrade() {
                                app.refresh_workspace_icons();
                            }
                        });
                    }
                })
            }),
            watcher: RefCell::new(None),
            clip_watcher: RefCell::new(None),
            plugin_runner: RefCell::new(None),
            updates: RefCell::new(None),
        });
        APP.with(|a| *a.borrow_mut() = Rc::downgrade(&app));
        app.st.borrow_mut().wallpaper = current_wallpaper_path(&app.cfg.borrow());
        app.wire_callbacks();
        app.apply_config();
        app.set_static_info();
        Ok(app)
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
