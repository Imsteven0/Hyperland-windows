//! komorebi integration: komorebi does the tiling, Caelestia stays the UI.
//!
//! * komorebi is started bare (`komorebic start`, **no** `--bar`, `--whkd` or
//!   `--ahk`): its status bar and hotkey daemon are never launched; our bar
//!   and our keyboard hook replace them.
//! * State comes from komorebi's event subscription: we host a named pipe and
//!   ask komorebi to connect to it (`komorebic subscribe-pipe caelestia`);
//!   every event carries the full state as one JSON line.
//! * Commands go through `komorebic` (fire-and-forget, no console window).

use std::{
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use serde_json::Value;
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        Storage::FileSystem::{PIPE_ACCESS_INBOUND, ReadFile},
        System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT},
    },
    core::HSTRING,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PIPE_NAME: &str = "caelestia";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct KWorkspace {
    pub name: String,
    pub occupied: bool,
    /// HWNDs of every window on the workspace (tiled, monocle, maximized, floating).
    pub windows: Vec<isize>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct KMonitor {
    /// Top-left of the monitor in physical pixels (matches our `Monitor.rect`).
    pub left: i32,
    pub top: i32,
    pub focused: usize,
    pub workspaces: Vec<KWorkspace>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct KState {
    pub monitors: Vec<KMonitor>,
    pub focused_monitor: usize,
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum KStatus {
    NotInstalled,
    Stopped,
    Running(KState),
}

/// `komorebic.exe` from PATH or the default install locations. Remembered
/// once found (every tiling shortcut runs it); until then each call looks
/// again, so installing komorebi while the shell runs just works.
pub fn komorebic() -> Option<PathBuf> {
    static FOUND: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    if let Some(p) = FOUND.get() {
        return Some(p.clone());
    }
    find_komorebic().map(|p| FOUND.get_or_init(|| p).clone())
}

fn find_komorebic() -> Option<PathBuf> {
    let from_path = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("komorebic.exe")).find(|p| p.is_file()));
    from_path.or_else(|| {
        let mut candidates = vec![PathBuf::from(r"C:\Program Files\komorebi\bin\komorebic.exe")];
        if let Some(local) = dirs::data_local_dir() {
            candidates.push(local.join(r"Microsoft\WinGet\Links\komorebic.exe"));
        }
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(r"scoop\shims\komorebic.exe"));
            candidates.push(home.join(r".cargo\bin\komorebic.exe"));
        }
        candidates.into_iter().find(|p| p.is_file())
    })
}

/// `komorebic` with komorebi's bin dir on PATH: `komorebic start` launches
/// `komorebi.exe` through a PATH lookup, and processes started before the
/// installer (e.g. this shell) still carry the old PATH.
fn komorebic_cmd(exe: &std::path::Path) -> Command {
    let mut cmd = Command::new(exe);
    if let Some(dir) = exe.parent() {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![dir.to_path_buf()];
        dirs.extend(std::env::split_paths(&path));
        if let Ok(joined) = std::env::join_paths(dirs) {
            cmd.env("PATH", joined);
        }
    }
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Runs `komorebic <args>` without blocking the caller.
pub fn command(args: &[&str]) {
    let Some(exe) = komorebic() else { return };
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    thread::spawn(move || {
        let r = komorebic_cmd(&exe).args(&args).stdout(Stdio::null()).stderr(Stdio::null()).status();
        if let Err(e) = r {
            log::warn!("komorebic {args:?}: {e}");
        }
    });
}

/// Runs a whitespace-separated komorebic command line, e.g. "focus left".
pub fn command_line(line: &str) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if !parts.is_empty() {
        command(&parts);
    }
}

fn run_blocking(args: &[&str]) -> Option<std::process::Output> {
    komorebic_cmd(&komorebic()?).args(args).stdin(Stdio::null()).output().ok()
}

/// Starts komorebi alone — never its bar (`--bar`) or hotkey daemon
/// (`--whkd`/`--ahk`) — with the static config we keep in sync. Fetches the
/// community app fixes first (once) so they're active from the start.
pub fn start() {
    thread::spawn(|| {
        let path = config_path();
        let asc = path.with_file_name("applications.json");
        if !asc.is_file() {
            let _ = run_blocking(&["fetch-app-specific-configuration"]);
        }
        let _ = link_app_specific_config();
        let args: Vec<String> = if path.is_file() { vec!["start".into(), "--config".into(), path.display().to_string()] } else { vec!["start".into()] };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match run_blocking(&args) {
            Some(o) if !o.status.success() => log::warn!("komorebic start: {}", String::from_utf8_lossy(&o.stderr).trim()),
            None => log::warn!("komorebic start could not be executed"),
            _ => {}
        }
    });
}

/// Points komorebi.json at applications.json once it exists.
fn link_app_specific_config() -> anyhow::Result<()> {
    let path = config_path();
    let asc = path.with_file_name("applications.json");
    if !asc.is_file() || !path.is_file() {
        return Ok(());
    }
    let mut cfg: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    if let Some(obj) = cfg.as_object_mut() {
        if !obj.contains_key("app_specific_configuration_path") {
            obj.insert("app_specific_configuration_path".into(), Value::String(asc.display().to_string()));
            std::fs::write(&path, serde_json::to_string_pretty(&cfg)? + "
")?;
        }
    }
    Ok(())
}

/// `komorebi.json` location (komorebi's own lookup: KOMOREBI_CONFIG_HOME, else the user profile).
pub fn config_path() -> PathBuf {
    std::env::var_os("KOMOREBI_CONFIG_HOME").map(PathBuf::from).or_else(dirs::home_dir).unwrap_or_default().join("komorebi.json")
}

/// The visual part of komorebi's config that follows Caelestia's dots/theme.
#[derive(Debug, Clone, PartialEq)]
pub struct Visuals {
    pub workspace_padding: i64,
    pub container_padding: i64,
    pub borders: bool,
    pub border_width: i64,
    pub border_offset: i64,
    /// `(kind, "#rrggbb")` for single / stack / monocle / unfocused / floating.
    pub colours: Vec<(&'static str, String)>,
    pub animations: bool,
    pub animation_ms: i64,
    /// Workspaces to create per monitor when komorebi.json defines none.
    pub workspaces: usize,
    pub monitors: usize,
    /// Executables whose windows use WS_EX_LAYERED (many Electron apps, e.g.
    /// Claude): komorebi skips those unless they're identified explicitly.
    /// `"*"` means every app (a single regex rule).
    pub layered_apps: Vec<String>,
}

/// What `sync_config` changed in `komorebi.json`.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Synced {
    /// Only applied by a full `replace-configuration` (which re-lays out windows).
    pub structure: bool,
    /// Border colours: applied live with `border-colour`, no reload.
    pub colours: bool,
}

/// Merges our managed keys into `komorebi.json`, keeping everything else the
/// user configured.
pub fn sync_config(v: &Visuals) -> anyhow::Result<Synced> {
    let path = config_path();
    let old_text = std::fs::read_to_string(&path).unwrap_or_default();
    // Community app fixes (fetched once with `komorebic fetch-app-specific-configuration`).
    let asc = Some(path.with_file_name("applications.json")).filter(|p| p.is_file());
    let (new_text, synced) = merge_visuals(&old_text, v, asc.as_deref())?;
    if synced != Synced::default() {
        std::fs::write(&path, new_text)?;
    }
    Ok(synced)
}

/// The new `komorebi.json` text for `v`, and what changed relative to
/// `old_text`. Only a structural change may trigger `replace-configuration`
/// (which re-lays out every window); a colour change never does.
fn merge_visuals(old_text: &str, v: &Visuals, app_specific: Option<&std::path::Path>) -> anyhow::Result<(String, Synced)> {
    use serde_json::json;
    let mut cfg: Value = if old_text.trim().is_empty() {
        json!({ "$schema": "https://raw.githubusercontent.com/LGUG2Z/komorebi/v0.1.41/schema.json" })
    } else {
        serde_json::from_str(&old_text)?
    };
    let old = cfg.clone();
    let obj = cfg.as_object_mut().ok_or_else(|| anyhow::anyhow!("komorebi.json is not an object"))?;
    obj.insert("default_workspace_padding".into(), json!(v.workspace_padding));
    obj.insert("default_container_padding".into(), json!(v.container_padding));
    obj.insert("border".into(), json!(v.borders));
    obj.insert("border_width".into(), json!(v.border_width));
    obj.insert("border_offset".into(), json!(v.border_offset));
    obj.insert("border_style".into(), json!("Rounded"));
    // Moving a window past the screen edge inserts it on the next monitor
    // (Hyprland's movewindow) instead of komorebi's default swap.
    obj.insert("cross_monitor_move_behaviour".into(), json!("Insert"));
    obj.insert("cross_boundary_behaviour".into(), json!("Monitor"));
    obj.insert("border_implementation".into(), json!("Komorebi"));
    let colours: serde_json::Map<String, Value> = v.colours.iter().map(|(k, c)| ((*k).to_owned(), json!(c))).collect();
    obj.insert("border_colours".into(), Value::Object(colours));
    // A komorebi `theme` would override border_colours; the shell owns colours now.
    obj.remove("theme");
    obj.insert("animation".into(), json!({ "enabled": v.animations, "duration": v.animation_ms, "style": "EaseOutCubic", "fps": 60 }));
    // Our surfaces are UI, never tiles (also enforced at runtime via ignore-rule).
    let rule = json!({ "kind": "Exe", "id": "caelestia.exe", "matching_strategy": "Equals" });
    let rules = obj.entry("ignore_rules").or_insert_with(|| json!([]));
    if let Some(arr) = rules.as_array_mut() {
        if !arr.iter().any(|r| r == &rule) {
            arr.push(rule);
        }
    }
    // Hyprland-like default: N named BSP workspaces on every monitor. Only
    // created when the user hasn't defined their own monitor layout.
    if v.workspaces > 0 {
        let ws: Vec<Value> = (1..=v.workspaces).map(|i| json!({ "name": i.to_string(), "layout": "BSP" })).collect();
        let generated = json!({ "workspaces": ws });
        let monitors = obj.entry("monitors").or_insert_with(|| json!([]));
        if let Some(arr) = monitors.as_array_mut() {
            // Only touch layouts we generated ourselves (or none at all): add an
            // entry for every connected monitor that doesn't have one yet.
            let ours = arr.iter().all(|m| m == &generated);
            if ours {
                while arr.len() < v.monitors.max(1) {
                    arr.push(generated.clone());
                }
            }
        }
    }
    // Layered (Electron) apps must be declared or komorebi never tiles them;
    // they're also tray apps whose window comes and goes.
    for app in &v.layered_apps {
        if app == "*" {
            // All apps: komorebi still requires a visible, captioned,
            // non-tool window, so overlays/tooltips/splashes stay unmanaged.
            let rule = json!({ "kind": "Exe", "id": ".*", "matching_strategy": "Regex" });
            if let Some(arr) = obj.entry("layered_applications").or_insert_with(|| json!([])).as_array_mut() {
                if !arr.contains(&rule) {
                    arr.push(rule);
                }
            }
            continue;
        }
        let rule = json!({ "kind": "Exe", "id": app, "matching_strategy": "Equals" });
        for key in ["layered_applications", "tray_and_multi_window_applications"] {
            if let Some(arr) = obj.entry(key).or_insert_with(|| json!([])).as_array_mut() {
                if !arr.contains(&rule) {
                    arr.push(rule.clone());
                }
            }
        }
    }
    if let Some(asc) = app_specific {
        if !obj.contains_key("app_specific_configuration_path") {
            obj.insert("app_specific_configuration_path".into(), json!(asc.display().to_string()));
        }
    }
    let new_text = serde_json::to_string_pretty(&cfg)? + "\n";
    if new_text == old_text {
        return Ok((new_text, Synced::default()));
    }
    let without_colours = |v: &Value| {
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("border_colours");
        }
        v
    };
    let synced = Synced {
        structure: old_text.trim().is_empty() || without_colours(&old) != without_colours(&cfg),
        colours: old.get("border_colours") != cfg.get("border_colours"),
    };
    Ok((new_text, synced))
}

/// Applies border colours to a running komorebi without reloading its
/// configuration (a reload would re-lay out every workspace).
pub fn apply_colours(colours: &[(&'static str, String)]) {
    for (kind, hex) in colours {
        let Some(rgb) = hex.strip_prefix('#').and_then(|h| u32::from_str_radix(h, 16).ok()) else { continue };
        let (r, g, b) = ((rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff);
        command(&["border-colour", "-w", kind, &r.to_string(), &g.to_string(), &b.to_string()]);
    }
}


/// Applies a changed static config to a running komorebi.
pub fn reload_config() {
    command(&["replace-configuration", &config_path().display().to_string()]);
}

pub fn stop() {
    command(&["stop"]);
}

/// Parses komorebi's serialized `State` (rings are `{elements, focused}`).
pub fn parse_state(v: &Value) -> KState {
    let ring = |v: &Value| -> (Vec<Value>, usize) {
        (v.get("elements").and_then(Value::as_array).cloned().unwrap_or_default(), v.get("focused").and_then(Value::as_u64).unwrap_or(0) as usize)
    };
    let (monitors, focused_monitor) = ring(&v["monitors"]);
    let monitors = monitors
        .iter()
        .map(|m| {
            let (workspaces, focused) = ring(&m["workspaces"]);
            KMonitor {
                left: m["size"]["left"].as_i64().unwrap_or(0) as i32,
                top: m["size"]["top"].as_i64().unwrap_or(0) as i32,
                focused,
                workspaces: workspaces
                    .iter()
                    .map(|w| {
                        let mut windows: Vec<isize> = Vec::new();
                        let mut push = |win: &Value| {
                            if let Some(h) = win["hwnd"].as_i64() {
                                if !windows.contains(&(h as isize)) {
                                    windows.push(h as isize);
                                }
                            }
                        };
                        let container = |c: &Value, push: &mut dyn FnMut(&Value)| {
                            for win in c["windows"]["elements"].as_array().into_iter().flatten() {
                                push(win);
                            }
                        };
                        for c in w["containers"]["elements"].as_array().into_iter().flatten() {
                            container(c, &mut push);
                        }
                        if !w["monocle_container"].is_null() {
                            container(&w["monocle_container"], &mut push);
                        }
                        if !w["maximized_window"].is_null() {
                            push(&w["maximized_window"]);
                        }
                        let floating = match &w["floating_windows"] {
                            Value::Array(a) => a.clone(),
                            other => other["elements"].as_array().cloned().unwrap_or_default(),
                        };
                        for win in &floating {
                            push(win);
                        }
                        let occupied = !windows.is_empty();
                        KWorkspace { name: w["name"].as_str().unwrap_or_default().to_owned(), occupied, windows }
                    })
                    .collect(),
            }
        })
        .collect();
    KState { monitors, focused_monitor, paused: v["is_paused"].as_bool().unwrap_or(false) }
}

/// Keeps a subscription alive and reports status changes.
pub fn spawn_watcher(on_status: impl Fn(KStatus) + Send + 'static) {
    thread::Builder::new()
        .name("komorebi".into())
        .spawn(move || {
            let mut last: Option<KStatus> = None;
            let mut report = |s: KStatus| {
                if last.as_ref() != Some(&s) {
                    last = Some(s.clone());
                    on_status(s);
                }
            };
            loop {
                if komorebic().is_none() {
                    report(KStatus::NotInstalled);
                    thread::sleep(Duration::from_secs(10));
                    continue;
                }
                // Prime with the current state (also tells us komorebi is up).
                match run_blocking(&["state"]).filter(|o| o.status.success()).and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok()) {
                    Some(v) => report(KStatus::Running(parse_state(&v))),
                    None => {
                        report(KStatus::Stopped);
                        thread::sleep(Duration::from_secs(3));
                        continue;
                    }
                }
                // Our windows are UI, never tiles.
                command(&["ignore-rule", "exe", "caelestia.exe"]);
                if let Err(e) = subscribe(&mut |s| report(KStatus::Running(s))) {
                    log::debug!("komorebi subscription ended: {e}");
                }
                thread::sleep(Duration::from_secs(1));
            }
        })
        .expect("spawn komorebi watcher");
}

/// Hosts `\\.\pipe\caelestia`, asks komorebi to connect, streams states.
fn subscribe(on_state: &mut dyn FnMut(KState)) -> anyhow::Result<()> {
    let name = HSTRING::from(format!(r"\\.\pipe\{PIPE_NAME}"));
    let pipe: HANDLE = unsafe { CreateNamedPipeW(&name, PIPE_ACCESS_INBOUND, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT, 1, 0, 1 << 20, 0, None) };
    if pipe.is_invalid() {
        anyhow::bail!("CreateNamedPipe failed");
    }
    struct Guard(HANDLE);
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                let _ = DisconnectNamedPipe(self.0);
                let _ = CloseHandle(self.0);
            }
        }
    }
    let guard = Guard(pipe);
    command(&["subscribe-pipe", PIPE_NAME]);
    unsafe { ConnectNamedPipe(guard.0, None) }.or_else(|e| {
        // ERROR_PIPE_CONNECTED: komorebi connected before we waited — fine.
        if e.code().0 as u32 == 0x8007_0217 { Ok(()) } else { Err(e) }
    })?;
    let mut buf = vec![0u8; 1 << 16];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let mut read = 0u32;
        unsafe { ReadFile(guard.0, Some(&mut buf), Some(&mut read), None) }?;
        if read == 0 {
            anyhow::bail!("pipe closed");
        }
        pending.extend_from_slice(&buf[..read as usize]);
        // Notifications are concatenated JSON objects (newline or not):
        // consume every complete value, keep a partial tail for the next read.
        let mut consumed = 0;
        let mut stream = serde_json::Deserializer::from_slice(&pending).into_iter::<Value>();
        loop {
            match stream.next() {
                Some(Ok(v)) => {
                    consumed = stream.byte_offset();
                    if v.get("state").is_some() {
                        on_state(parse_state(&v["state"]));
                    }
                }
                Some(Err(e)) if e.is_eof() => break,
                Some(Err(_)) => {
                    consumed = pending.len(); // garbage: resync on the next message
                    break;
                }
                None => break,
            }
        }
        pending.drain(..consumed);
    }
}

/// Default tiling binds, modelled on komorebi's sample whkdrc so existing
/// komorebi users feel at home. Overridable via `windows.komorebi.binds`.
pub fn default_binds() -> Vec<(&'static str, String)> {
    let mut b: Vec<(&'static str, String)> = vec![
        ("Alt+H", "focus left".into()),
        ("Alt+J", "focus down".into()),
        ("Alt+K", "focus up".into()),
        ("Alt+L", "focus right".into()),
        ("Alt+Shift+H", "move left".into()),
        ("Alt+Shift+J", "move down".into()),
        ("Alt+Shift+K", "move up".into()),
        ("Alt+Shift+L", "move right".into()),
        ("Alt+Q", "close".into()),
        ("Alt+M", "minimize".into()),
        ("Alt+T", "toggle-float".into()),
        ("Alt+Shift+F", "toggle-monocle".into()),
        ("Alt+X", "flip-layout horizontal".into()),
        ("Alt+Y", "flip-layout vertical".into()),
        ("Alt+Shift+R", "retile".into()),
        ("Alt+P", "toggle-pause".into()),
        ("Alt+Comma", "cycle-layout previous".into()),
        ("Alt+Period", "cycle-layout next".into()),
        ("Alt+Shift+M", "toggle-maximize".into()),
        ("Alt+Shift+Enter", "promote".into()),
        // monitors
        ("Alt+O", "cycle-monitor next".into()),
        ("Alt+Shift+O", "cycle-move-to-monitor next".into()),
        // workspaces
        ("Alt+W", "focus-last-workspace".into()),
        ("Alt+Shift+N", "cycle-move-to-workspace next".into()),
        ("Alt+Shift+B", "cycle-move-to-workspace previous".into()),
        // resize the focused window
        ("Alt+Plus", "resize-axis horizontal increase".into()),
        ("Alt+Minus", "resize-axis horizontal decrease".into()),
        ("Alt+Shift+Plus", "resize-axis vertical increase".into()),
        ("Alt+Shift+Minus", "resize-axis vertical decrease".into()),
    ];
    const KEYS: [&str; 8] = ["Alt+1", "Alt+2", "Alt+3", "Alt+4", "Alt+5", "Alt+6", "Alt+7", "Alt+8"];
    const MOVE: [&str; 8] = ["Alt+Shift+1", "Alt+Shift+2", "Alt+Shift+3", "Alt+Shift+4", "Alt+Shift+5", "Alt+Shift+6", "Alt+Shift+7", "Alt+Shift+8"];
    const SEND: [&str; 8] = ["Alt+Ctrl+1", "Alt+Ctrl+2", "Alt+Ctrl+3", "Alt+Ctrl+4", "Alt+Ctrl+5", "Alt+Ctrl+6", "Alt+Ctrl+7", "Alt+Ctrl+8"];
    for i in 0..8 {
        b.push((KEYS[i], format!("focus-workspace {i}")));
        // move = the window goes there and you follow it; send = you stay.
        b.push((MOVE[i], format!("move-to-workspace {i}")));
        b.push((SEND[i], format!("send-to-workspace {i}")));
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn visuals() -> Visuals {
        Visuals {
            workspace_padding: 10,
            container_padding: 10,
            borders: true,
            border_width: 3,
            border_offset: 0,
            colours: vec![("single", "#b5c4ff".into()), ("unfocused", "#45464f".into())],
            animations: true,
            animation_ms: 250,
            workspaces: 3,
            monitors: 2,
            layered_apps: vec!["*".into(), "claude.exe".into()],
        }
    }

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn first_sync_writes_everything_and_is_structural() {
        let (text, synced) = merge_visuals("", &visuals(), None).unwrap();
        assert_eq!(synced, Synced { structure: true, colours: true });
        let cfg = json(&text);
        assert_eq!(cfg["default_workspace_padding"], 10);
        assert_eq!(cfg["border_colours"]["single"], "#b5c4ff");
        assert_eq!(cfg["monitors"].as_array().unwrap().len(), 2);
        assert_eq!(cfg["monitors"][0]["workspaces"].as_array().unwrap().len(), 3);
        assert!(cfg["ignore_rules"].as_array().unwrap().iter().any(|r| r["id"] == "caelestia.exe"));
        assert!(cfg["layered_applications"].as_array().unwrap().iter().any(|r| r["id"] == ".*"));
    }

    #[test]
    fn syncing_twice_changes_nothing() {
        let (text, _) = merge_visuals("", &visuals(), None).unwrap();
        let (again, synced) = merge_visuals(&text, &visuals(), None).unwrap();
        assert_eq!(synced, Synced::default());
        assert_eq!(again, text);
    }

    /// A wallpaper/scheme change only recolours borders: komorebi must not
    /// reload its configuration, which re-lays out (reorders) every window.
    #[test]
    fn colour_changes_are_not_structural() {
        let (text, _) = merge_visuals("", &visuals(), None).unwrap();
        let mut v = visuals();
        v.colours[0].1 = "#ff8800".into();
        let (_, synced) = merge_visuals(&text, &v, None).unwrap();
        assert_eq!(synced, Synced { structure: false, colours: true });
    }

    #[test]
    fn layout_changes_are_structural() {
        let (text, _) = merge_visuals("", &visuals(), None).unwrap();
        let mut v = visuals();
        v.container_padding = 20;
        let (_, synced) = merge_visuals(&text, &v, None).unwrap();
        assert_eq!(synced, Synced { structure: true, colours: false });
    }

    #[test]
    fn keeps_the_users_own_settings_and_layout() {
        let user = r#"{
            "float_rules": [{ "kind": "Exe", "id": "calc.exe", "matching_strategy": "Equals" }],
            "theme": { "palette": "Catppuccin" },
            "monitors": [{ "workspaces": [{ "name": "code", "layout": "VerticalStack" }] }],
            "ignore_rules": [{ "kind": "Exe", "id": "caelestia.exe", "matching_strategy": "Equals" }]
        }"#;
        let (text, _) = merge_visuals(user, &visuals(), None).unwrap();
        let cfg = json(&text);
        assert_eq!(cfg["float_rules"][0]["id"], "calc.exe");
        assert!(cfg.get("theme").is_none(), "a komorebi theme would override our border colours");
        assert_eq!(cfg["monitors"].as_array().unwrap().len(), 1, "user layouts are never extended");
        assert_eq!(cfg["monitors"][0]["workspaces"][0]["name"], "code");
        assert_eq!(cfg["ignore_rules"].as_array().unwrap().len(), 1, "no duplicate rule");
    }

    #[test]
    fn adds_monitors_to_our_generated_layout_only() {
        let (text, _) = merge_visuals("", &Visuals { monitors: 1, ..visuals() }, None).unwrap();
        assert_eq!(json(&text)["monitors"].as_array().unwrap().len(), 1);
        let (text, synced) = merge_visuals(&text, &Visuals { monitors: 3, ..visuals() }, None).unwrap();
        assert_eq!(json(&text)["monitors"].as_array().unwrap().len(), 3);
        assert!(synced.structure);
    }

    #[test]
    fn rejects_invalid_files_instead_of_overwriting_them() {
        assert!(merge_visuals("{ not json", &visuals(), None).is_err());
        assert!(merge_visuals("[1, 2]", &visuals(), None).is_err());
    }

    #[test]
    fn parses_rings_and_occupancy() {
        let v = json!({
            "is_paused": false,
            "monitors": { "focused": 1, "elements": [
                { "size": { "left": 0, "top": 0, "right": 2560, "bottom": 1440 },
                  "workspaces": { "focused": 0, "elements": [
                      { "name": "I", "containers": { "elements": [ { "windows": { "elements": [ { "hwnd": 42 } ] } } ], "focused": 0 }, "floating_windows": [], "monocle_container": null, "maximized_window": null },
                      { "name": "II", "containers": { "elements": [], "focused": 0 }, "floating_windows": [], "monocle_container": null, "maximized_window": null }
                  ] } },
                { "size": { "left": 2560, "top": 0, "right": 2560, "bottom": 1440 },
                  "workspaces": { "focused": 1, "elements": [ { "name": "I", "containers": { "elements": [] } }, { "name": "II", "containers": { "elements": [] }, "floating_windows": { "elements": [ { "hwnd": 7 } ] } } ] } }
            ] }
        });
        let s = parse_state(&v);
        assert_eq!(s.focused_monitor, 1);
        assert_eq!(s.monitors[0].workspaces.iter().map(|w| w.occupied).collect::<Vec<_>>(), vec![true, false]);
        assert_eq!(s.monitors[1].left, 2560);
        assert_eq!(s.monitors[1].focused, 1);
        assert!(s.monitors[1].workspaces[1].occupied);
        assert_eq!(s.monitors[0].workspaces[0].windows, vec![42]);
        assert_eq!(s.monitors[1].workspaces[1].windows, vec![7]);
    }
}
