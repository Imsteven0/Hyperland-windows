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
    Action { id: "calc", name: "Calculator", description: "Evaluate maths as you type (=2+2)", icon: "calculate", dangerous: false },
    Action { id: "emoji", name: "Emoji", description: "Search and copy an emoji (:fire)", icon: "mood", dangerous: false },
    Action { id: "clipboard", name: "Clipboard", description: "Paste from the clipboard history (;)", icon: "content_paste", dangerous: false },
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

/// What the launcher shows for a query, chosen by its prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Apps,
    Actions,
    Calc,
    Emoji,
    Clipboard,
}

/// Launcher prefixes from shell.json (`launcher.*Prefix`); an empty one is off.
#[derive(Debug, Clone, Default)]
pub struct Prefixes {
    pub actions: String,
    pub calc: String,
    pub emoji: String,
    pub clipboard: String,
}

impl Prefixes {
    pub fn get(&self, mode: Mode) -> &str {
        match mode {
            Mode::Apps => "",
            Mode::Actions => &self.actions,
            Mode::Calc => &self.calc,
            Mode::Emoji => &self.emoji,
            Mode::Clipboard => &self.clipboard,
        }
    }

    /// Splits `query` into its mode and the text after the prefix. The
    /// longest matching prefix wins, so ">" and ">>" can coexist.
    pub fn split<'a>(&self, query: &'a str) -> (Mode, &'a str) {
        [Mode::Actions, Mode::Calc, Mode::Emoji, Mode::Clipboard]
            .into_iter()
            .map(|m| (m, self.get(m)))
            .filter(|(_, p)| !p.is_empty() && query.starts_with(p))
            .max_by_key(|(_, p)| p.len())
            .map(|(m, p)| (m, &query[p.len()..]))
            .unwrap_or((Mode::Apps, query))
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn prefixes() -> Prefixes {
        Prefixes { actions: ">".into(), calc: "=".into(), emoji: ":".into(), clipboard: ";".into() }
    }

    #[test]
    fn splits_queries_by_prefix() {
        let p = prefixes();
        assert_eq!(p.split("firefox"), (Mode::Apps, "firefox"));
        assert_eq!(p.split(">wall"), (Mode::Actions, "wall"));
        assert_eq!(p.split("=2+2"), (Mode::Calc, "2+2"));
        assert_eq!(p.split(":fuego"), (Mode::Emoji, "fuego"));
        assert_eq!(p.split(";"), (Mode::Clipboard, ""));
        assert_eq!(p.split(""), (Mode::Apps, ""));
    }

    #[test]
    fn longest_prefix_wins_and_empty_disables() {
        let p = Prefixes { actions: ">".into(), calc: ">>".into(), emoji: String::new(), clipboard: ";".into() };
        assert_eq!(p.split(">>1+1"), (Mode::Calc, "1+1"));
        assert_eq!(p.split(">x"), (Mode::Actions, "x"));
        assert_eq!(p.split(":fire"), (Mode::Apps, ":fire"));
    }

    #[test]
    fn fuzzy_prefers_prefix_matches() {
        let mut f = Fuzzy::default();
        let names = ["Visual Studio Code", "Code Writer", "Calculator"];
        assert_eq!(f.rank("code", names.iter().copied(), 10).first(), Some(&1));
        assert_eq!(f.rank("calc", names.iter().copied(), 10), vec![2]);
        assert!(f.rank("zzz", names.iter().copied(), 10).is_empty());
    }

    #[test]
    fn action_ids_are_unique() {
        for (i, a) in ACTIONS.iter().enumerate() {
            assert!(ACTIONS[..i].iter().all(|b| b.id != a.id), "duplicate action {}", a.id);
        }
    }
}
