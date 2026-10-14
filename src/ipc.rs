//! Tiny IPC so the running shell can be driven from the command line, in the
//! spirit of `caelestia shell drawers toggle launcher`:
//!
//!   caelestia toggle launcher|dashboard|utilities|session|sidebar|settings
//!   caelestia dashboard 0|1|2|3   (overview, media, performance, weather)
//!   caelestia launcher calc|emoji|clipboard
//!   caelestia workspace 1..8|next|prev
//!   caelestia reload
//!   caelestia quit
//!
//! Each command is a named auto-reset event; no sockets, no parsing.

use std::thread;

use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
        System::Threading::{CreateEventW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent, WaitForMultipleObjects},
    },
    core::HSTRING,
};

pub const COMMANDS: [&str; 42] = [
    "quit",
    "reload",
    "toggle:launcher",
    "toggle:dashboard",
    "toggle:utilities",
    "toggle:session",
    "toggle:sidebar",
    "toggle:settings",
    // `caelestia dashboard <tab>` opens the dashboard on a tab.
    "dashboard:0",
    "dashboard:1",
    "dashboard:2",
    "dashboard:3",
    // `caelestia popout audio|network|bluetooth|calendar` (pinned until the pointer leaves)
    "popout:audio",
    "popout:network",
    "popout:bluetooth",
    "popout:calendar",
    // `caelestia settings <page>`: 0 style, 1 network, 2 devices, 3 audio, 4 updates,
    // 5 plugins, 6 panels, 7 services (weather location), 8 language, 9 about
    "settings:0",
    "settings:1",
    "settings:2",
    "settings:3",
    "settings:4",
    "settings:5",
    "settings:6",
    "settings:7",
    "settings:8",
    "settings:9",
    // `caelestia picker scheme|variant|opacity`: live-preview carousels
    "picker:scheme",
    "picker:variant",
    "picker:opacity",
    // `caelestia launcher calc|emoji|clipboard`: launcher in that mode
    "launcher:calc",
    "launcher:emoji",
    "launcher:clipboard",
    // `caelestia workspace <n>|next|prev` (like `hyprctl dispatch workspace`),
    // with the same transition as Alt+1..8.
    "workspace:1",
    "workspace:2",
    "workspace:3",
    "workspace:4",
    "workspace:5",
    "workspace:6",
    "workspace:7",
    "workspace:8",
    "workspace:next",
    "workspace:prev",
];

/// Optional instance suffix (`CAELESTIA_INSTANCE`) so a dev build can run
/// next to an installed shell.
pub fn instance() -> String {
    std::env::var("CAELESTIA_INSTANCE").map(|s| format!(".{s}")).unwrap_or_default()
}

fn event_name(cmd: &str) -> HSTRING {
    HSTRING::from(format!("Local\\Caelestia{}.{}", instance(), cmd.replace(':', ".")))
}

/// Parses CLI args into a command, if any.
pub fn parse_args(args: &[String]) -> Option<Result<&'static str, String>> {
    let joined = match args {
        [] => return None,
        [a] => a.clone(),
        [a, b, ..] => format!("{a}:{b}"),
    };
    Some(COMMANDS.iter().copied().find(|c| *c == joined).ok_or_else(|| format!("unknown command '{}'. Valid: {}", args.join(" "), COMMANDS.join(", ").replace(':', " "))))
}

/// Signals the running instance. Returns false if no shell is running.
pub fn send(cmd: &str) -> bool {
    unsafe {
        match OpenEventW(EVENT_MODIFY_STATE, false, &event_name(cmd)) {
            Ok(h) => {
                let ok = SetEvent(h).is_ok();
                let _ = CloseHandle(h);
                ok
            }
            Err(_) => false,
        }
    }
}

pub fn listen(on_command: impl Fn(&'static str) + Send + 'static) {
    let handles: Vec<HANDLE> = COMMANDS.iter().filter_map(|c| unsafe { CreateEventW(None, false, false, &event_name(c)).ok() }).collect();
    if handles.len() != COMMANDS.len() {
        log::warn!("IPC unavailable");
        return;
    }
    let raw: Vec<isize> = handles.iter().map(|h| h.0 as isize).collect();
    thread::Builder::new()
        .name("ipc".into())
        .spawn(move || {
            let handles: Vec<HANDLE> = raw.into_iter().map(|h| HANDLE(h as *mut _)).collect();
            loop {
                let r = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                let idx = r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
                match COMMANDS.get(idx) {
                    Some(cmd) => on_command(cmd),
                    None => return,
                }
            }
        })
        .expect("spawn ipc");
}
