//! Performance mode: drops the blur, the animations and hover-to-open while
//! on battery or while a full-screen game/video has the foreground
//! (`windows.performance` in shell.json). This module only decides; the
//! shell applies the result and the platform layer gathers the inputs.

/// `windows.performance.mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// Switch automatically (battery / full screen).
    Auto,
    /// Always reduced effects.
    On,
    /// Never.
    Off,
}

impl Setting {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "on" | "always" | "true" => Setting::On,
            "off" | "never" | "false" => Setting::Off,
            _ => Setting::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub setting: Setting,
    pub on_battery: bool,
    pub on_fullscreen: bool,
}

/// What the system looks like right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Inputs {
    /// Running on battery (unplugged).
    pub on_battery: bool,
    /// Windows' battery/energy saver is on.
    pub battery_saver: bool,
    /// A full-screen app (game, video, presentation) has the foreground.
    pub fullscreen: bool,
}

/// Why effects are reduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Forced,
    Fullscreen,
    Battery,
}

impl Reason {
    pub fn id(self) -> &'static str {
        match self {
            Reason::Forced => "forced",
            Reason::Fullscreen => "fullscreen",
            Reason::Battery => "battery",
        }
    }
}

/// `None` = full effects. Full screen wins over battery: it also disables
/// hover-to-open, so a mouse at the top edge in a game opens nothing.
pub fn decide(policy: Policy, inputs: Inputs) -> Option<Reason> {
    match policy.setting {
        Setting::Off => None,
        Setting::On => Some(Reason::Forced),
        Setting::Auto if policy.on_fullscreen && inputs.fullscreen => Some(Reason::Fullscreen),
        Setting::Auto if policy.on_battery && (inputs.on_battery || inputs.battery_saver) => Some(Reason::Battery),
        Setting::Auto => None,
    }
}

/// Enters a reduced state at once but leaves it only after `release`
/// consecutive checks agree, so alt-tabbing out of a game for a second
/// doesn't flash the blur and animations back on.
#[derive(Debug, Clone)]
pub struct Tracker {
    state: Option<Reason>,
    pending_exit: u32,
    release: u32,
}

impl Tracker {
    pub fn new(release: u32) -> Self {
        Self { state: None, pending_exit: 0, release: release.max(1) }
    }

    pub fn state(&self) -> Option<Reason> {
        self.state
    }

    /// Feeds one check; returns the new state when it changed.
    pub fn update(&mut self, wanted: Option<Reason>) -> Option<Option<Reason>> {
        if wanted == self.state {
            self.pending_exit = 0;
            return None;
        }
        // Forced on/off and switching between reasons apply immediately;
        // only "back to full effects" after an automatic reason waits.
        let delay = wanted.is_none() && self.state.is_some_and(|r| r != Reason::Forced);
        if delay {
            self.pending_exit += 1;
            if self.pending_exit < self.release {
                return None;
            }
        }
        self.pending_exit = 0;
        self.state = wanted;
        Some(wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auto() -> Policy {
        Policy { setting: Setting::Auto, on_battery: true, on_fullscreen: true }
    }

    #[test]
    fn parses_settings() {
        assert_eq!(Setting::parse("On"), Setting::On);
        assert_eq!(Setting::parse("off"), Setting::Off);
        assert_eq!(Setting::parse("auto"), Setting::Auto);
        assert_eq!(Setting::parse("whatever"), Setting::Auto);
    }

    #[test]
    fn decides_by_policy() {
        let idle = Inputs::default();
        let battery = Inputs { on_battery: true, ..idle };
        let saver = Inputs { battery_saver: true, ..idle };
        let game = Inputs { fullscreen: true, on_battery: true, ..idle };
        assert_eq!(decide(auto(), idle), None);
        assert_eq!(decide(auto(), battery), Some(Reason::Battery));
        assert_eq!(decide(auto(), saver), Some(Reason::Battery));
        assert_eq!(decide(auto(), game), Some(Reason::Fullscreen));
        assert_eq!(decide(Policy { on_fullscreen: false, ..auto() }, game), Some(Reason::Battery));
        assert_eq!(decide(Policy { on_battery: false, ..auto() }, battery), None);
        assert_eq!(decide(Policy { setting: Setting::On, ..auto() }, idle), Some(Reason::Forced));
        assert_eq!(decide(Policy { setting: Setting::Off, ..auto() }, game), None);
    }

    #[test]
    fn tracker_enters_fast_and_leaves_slowly() {
        let mut t = Tracker::new(2);
        assert_eq!(t.update(None), None);
        assert_eq!(t.update(Some(Reason::Fullscreen)), Some(Some(Reason::Fullscreen)));
        // A one-check blip (alt-tab) keeps the reduced state.
        assert_eq!(t.update(None), None);
        assert_eq!(t.update(Some(Reason::Fullscreen)), None);
        assert_eq!(t.update(None), None);
        assert_eq!(t.update(None), Some(None));
        assert_eq!(t.state(), None);
    }

    #[test]
    fn tracker_applies_forced_changes_immediately() {
        let mut t = Tracker::new(3);
        assert_eq!(t.update(Some(Reason::Forced)), Some(Some(Reason::Forced)));
        assert_eq!(t.update(None), Some(None));
        assert_eq!(t.update(Some(Reason::Battery)), Some(Some(Reason::Battery)));
        assert_eq!(t.update(Some(Reason::Fullscreen)), Some(Some(Reason::Fullscreen)));
    }
}
