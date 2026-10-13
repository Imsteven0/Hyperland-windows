//! Launcher back-end: app index (built off-thread), `>` actions and fuzzy
//! ranking with nucleo (the matcher used by Helix).

use std::thread;

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};

use crate::platform::shell_items::{AppRecord, enumerate_apps};

pub fn spawn_index(hidden: Vec<String>, icon_px: i32, on_done: impl FnOnce(Vec<AppRecord>) + Send + 'static) {
    thread::Builder::new()
        .name("app-index".into())
        .spawn(move || {
            crate::platform::com_init(true);
            let started = std::time::Instant::now();
            let apps = enumerate_apps(&hidden, icon_px);
            log::info!("indexed {} apps in {:?}", apps.len(), started.elapsed());
            on_done(apps);
        })
        .expect("spawn app index");
}

#[derive(Debug, Clone, Copy)]
pub struct Action {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
    pub dangerous: bool,
}

pub const ACTIONS: &[Action] = &[
    Action { id: "wallpaper", name: "Wallpaper", description: "Change the current wallpaper", icon: "image", dangerous: false },
    Action { id: "random-wallpaper", name: "Random", description: "Switch to a random wallpaper", icon: "casino", dangerous: false },
    Action { id: "light", name: "Light", description: "Change the scheme to light mode", icon: "light_mode", dangerous: false },
    Action { id: "dark", name: "Dark", description: "Change the scheme to dark mode", icon: "dark_mode", dangerous: false },
    Action { id: "scheme", name: "Scheme", description: "Pick a colour scheme (live preview)", icon: "palette", dangerous: false },
    Action { id: "variant", name: "Variant", description: "Pick the dynamic scheme variant (live preview)", icon: "colors", dangerous: false },
    Action { id: "transparency", name: "Transparency", description: "Pick the panel opacity theme (live preview)", icon: "opacity", dangerous: false },
    Action { id: "settings", name: "Settings", description: "Open the shell settings", icon: "settings", dangerous: false },
    Action { id: "dashboard", name: "Dashboard", description: "Open the dashboard", icon: "dashboard", dangerous: false },
    Action { id: "reload", name: "Reload", description: "Reload shell.json and scheme.json", icon: "refresh", dangerous: false },
    Action { id: "lock", name: "Lock", description: "Lock the session", icon: "lock", dangerous: false },
    Action { id: "sleep", name: "Sleep", description: "Suspend the computer", icon: "bedtime", dangerous: true },
    Action { id: "logout", name: "Logout", description: "Sign out of the session", icon: "logout", dangerous: true },
    Action { id: "reboot", name: "Reboot", description: "Restart the computer", icon: "cached", dangerous: true },
    Action { id: "shutdown", name: "Shutdown", description: "Power off the computer", icon: "power_settings_new", dangerous: true },
    Action { id: "quit", name: "Quit shell", description: "Exit Caelestia and free the reserved screen space", icon: "close", dangerous: false },
];

pub struct Fuzzy {
    matcher: Matcher,
    buf: Vec<char>,
}

impl Default for Fuzzy {
    fn default() -> Self {
        Self { matcher: Matcher::new(Config::DEFAULT), buf: Vec::new() }
    }
}

impl Fuzzy {
    /// Returns indices of `haystacks` that match `query`, best first.
    pub fn rank<'a>(&mut self, query: &str, haystacks: impl Iterator<Item = &'a str>, limit: usize) -> Vec<usize> {
        let pattern = Pattern::new(query, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy);
        let mut scored: Vec<(u32, usize)> = haystacks
            .enumerate()
            .filter_map(|(i, h)| {
                let s = pattern.score(Utf32Str::new(h, &mut self.buf), &mut self.matcher)?;
                // Prefix matches feel right in a launcher: boost them.
                let boost = if h.to_lowercase().starts_with(&query.to_lowercase()) { 1000 } else { 0 };
                Some((s + boost, i))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().take(limit).map(|(_, i)| i).collect()
    }
}
