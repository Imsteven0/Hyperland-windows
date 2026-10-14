// No console window in release builds (logs go to %LOCALAPPDATA%\caelestia\shell.log).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod app;
mod config;
mod fonts;
mod hotkeys;
mod i18n;
mod ipc;
mod platform;
mod schemes;
mod services;
mod theme;

use anyhow::{Context, Result};
use windows::{
    Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError},
    Win32::System::Threading::CreateMutexW,
};

fn main() -> Result<()> {
    // `caelestia toggle launcher`, `caelestia quit`, ... talk to the running shell.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("diag") {
        return diag();
    }
    if let Some(cmd) = ipc::parse_args(&args) {
        let cmd = cmd.map_err(anyhow::Error::msg)?;
        if !ipc::send(cmd) {
            anyhow::bail!("Caelestia is not running");
        }
        return Ok(());
    }

    let paths = config::Paths::resolve();
    init_logging(&paths);
    // A crash must not leave app windows click-through (see platform::ghosts).
    let ghosts_file = paths.state_dir.join("click-through.json");
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        platform::ghosts::restore_from_disk(&ghosts_file);
        default_hook(info);
    }));

    // One shell per session: the second instance would fight over the AppBars.
    let mutex_name = windows::core::HSTRING::from(format!("Local\\CaelestiaWindowsShell{}", ipc::instance()));
    let _mutex = unsafe { CreateMutexW(None, true, &mutex_name) }.context("creating instance mutex")?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS && another_instance_visible() {
        log::warn!("Caelestia is already running");
        return Ok(());
    }

    platform::window::init_dpi_awareness();
    platform::com_init(true);

    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("femtovg".into())
        .with_winit_window_attributes_hook(|attrs| {
            use slint::winit_030::winit::{platform::windows::WindowAttributesExtWindows, window::WindowLevel};
            // Shell surfaces: no taskbar button, never activate on creation.
            attrs.with_skip_taskbar(true).with_active(false).with_window_level(WindowLevel::AlwaysOnTop)
        })
        .select()
        .context("selecting the winit/femtovg backend")?;

    let families = fonts::register(&paths.config_dir);
    log::info!("registered fonts: {families:?}");
    if !families.iter().any(|f| f.contains("Material Symbols")) {
        log::warn!("Material Symbols Rounded not found — run scripts/fetch-fonts.ps1 or place the font in {}", paths.config_dir.join("fonts").display());
    }

    let shell = app::Caelestia::new(paths)?;
    shell.start()?;
    slint::run_event_loop_until_quit()?;
    shell.shutdown();
    log::info!("bye");
    // Leave without running DLL detach: some OpenGL drivers (nvoglv64) deadlock
    // in DLL_PROCESS_DETACH while GL contexts are alive, leaving an
    // unkillable process. Everything visible to the system is released above.
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
    Ok(())
}

/// `caelestia diag`: prints what the shell sees (audio endpoints, Bluetooth
/// devices) to help debug hardware integration without starting the UI.
fn diag() -> Result<()> {
    platform::com_init(false);
    println!("== audio outputs");
    for d in platform::audio::devices(false) {
        println!("  {} {}", if d.default { "*" } else { " " }, d.name);
    }
    println!("== audio inputs");
    for d in platform::audio::devices(true) {
        println!("  {} {}", if d.default { "*" } else { " " }, d.name);
    }
    let a = platform::audio::state();
    println!("== levels: out {:.0}% muted={} | mic {:.0}% muted={}", a.volume * 100.0, a.muted, a.mic_volume * 100.0, a.mic_muted);
    println!("== do not disturb: profile={:?}", platform::focus::profile());
    if let Some(q) = std::env::args().nth(2) {
        println!("== places for {q:?}");
        match services::weather::search_places(&q, "es", "CR") {
            Ok(ps) => ps.iter().for_each(|p| println!("  {:<3} {:<40} {}", p.country_code, p.label(), p.coords())),
            Err(e) => println!("  error: {e:#}"),
        }
    }
    println!("== wi-fi");
    let allowed = platform::wifi::request_access();
    match platform::wifi::adapter() {
        None => {
            println!("  no adapter (access allowed: {allowed}): saved networks only");
            for n in platform::wifi::saved_networks() {
                println!("  [{}] {}", if n.connected { "x" } else { " " }, n.ssid);
            }
        }
        Some(a) => {
            let scanned = platform::wifi::scan(&a);
            let nets = platform::wifi::networks(&a);
            println!("  access allowed: {allowed}, scan ok: {scanned}, {} networks", nets.len());
            for n in nets.iter().take(12) {
                println!("  [{}] {:<32} bars={} secured={} saved={}", if n.connected { "x" } else { " " }, n.ssid, n.bars, n.secured, n.saved);
            }
        }
    }
    println!("== bluetooth (paired)");
    for d in platform::bluetooth::paired_devices() {
        let ks = d.container.map(platform::audio::bluetooth_audio_controls).unwrap_or(0);
        let active = d.container.is_some_and(platform::audio::audio_endpoint_active);
        println!("  [{}] {:<32} connected={} audio={} audio_active={active} icon={} ks_filters={ks}", if d.connected { "x" } else { " " }, d.name, d.connected, d.audio, d.icon);
    }
    for (kind, name, connected, container) in platform::bluetooth::raw_entries() {
        println!("    raw {kind:<7} {name:<32} connected={connected} container={container:?}");
    }
    Ok(())
}

/// A mutex alone isn't enough: a previous instance stuck in driver teardown
/// keeps it alive. Only a visible bar means a shell is really running.
fn another_instance_visible() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, IsWindowVisible};
    unsafe { FindWindowW(None, windows::core::w!("caelestia-bar")).is_ok_and(|h| IsWindowVisible(h).as_bool()) }
}

fn init_logging(paths: &config::Paths) {
    let mut builder = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,naga=warn,wgpu=warn"));
    if !cfg!(debug_assertions) {
        if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(paths.cache_dir.join("shell.log")).or_else(|_| {
            std::fs::create_dir_all(&paths.cache_dir)?;
            std::fs::File::create(paths.cache_dir.join("shell.log"))
        }) {
            builder.target(env_logger::Target::Pipe(Box::new(file)));
        }
    }
    builder.init();
}
