//! Workspace switch animation with komorebi, like Caelestia on Hyprland.
//!
//! komorebi switches instantly, so the shell stages the transition itself on
//! an overlay of live DWM thumbnails (`platform::workspace_overlay`):
//!
//! - Shortcuts and bar clicks that name the destination (`focus-workspace N`,
//!   next/previous): the stage covers the monitor and the slide starts at
//!   once, with the incoming windows taken from komorebi's last state (DWM
//!   mirrors them although they're cloaked). komorebi switches underneath.
//! - Switches whose result only komorebi knows (moving a window along, the
//!   last workspace): the stage covers the monitor and waits for komorebi's
//!   new state before sliding.
//! - Switches made elsewhere (komorebic, a window taking focus): the slide
//!   starts from komorebi's notification, mirroring the previous state.
//!
//! The stage goes away once the slide has played *and* komorebi has
//! switched, revealing the real windows already in place.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use windows::Win32::Foundation::HWND;

use super::*;
use crate::{
    platform::workspace_overlay::{self as stage, Layer, Overlay},
    services::workspace_anim::{self as wsa, Style},
};

/// Longest wait for komorebi to confirm a switch the stage is covering.
const SWITCH_TIMEOUT: Duration = Duration::from_millis(900);

/// A transition in progress.
pub(super) struct WsTransition {
    /// komorebi monitor index.
    monitor: usize,
    from: usize,
    /// Workspace being slid in, once known.
    to: Option<usize>,
    generation: u64,
    /// Set while the slide plays.
    cancel: Option<Arc<AtomicBool>>,
    /// komorebi has reported the new workspace.
    switched: bool,
    /// The slide reached its end.
    played: bool,
}

impl Caelestia {
    /// Style and length, or `None` when the animation is off (setting,
    /// animation speed 0, performance mode forced or on battery).
    /// Performance mode for a full-screen app doesn't count: switching
    /// workspaces is how you leave (or return to) that video or game, and
    /// Hyprland animates it too. Its 2 s re-check made switches go flat
    /// for a while after leaving a full-screen video.
    fn ws_anim_settings(&self) -> Option<(Style, Duration)> {
        let style = Style::parse(&self.cfg.borrow().str("windows.komorebi.workspaceAnimation"))?;
        let scale = match self.perf_reason() {
            Some(PerfReason::Fullscreen) => self.cfg.borrow().f32("appearance.anim.durations.scale"),
            _ => self.anim_scale(),
        };
        Some((style, wsa::duration(scale)?))
    }

    /// Before running a komorebic command: stage the transition if it is
    /// going to switch the focused monitor's workspace.
    pub(super) fn ws_anim_before_command(&self, command: &str) {
        let Some(switch) = wsa::switch_of(command) else { return };
        let KStatus::Running(state) = self.st.borrow().komorebi.clone() else { return };
        let km = state.focused_monitor;
        let Some(m) = state.monitors.get(km) else { return };
        let target = switch.target(m.focused, m.workspaces.len());
        if let Some(n) = target
            && (n == m.focused || n >= m.workspaces.len())
        {
            return; // nothing will move
        }
        self.ws_anim_begin(&state, km, target);
    }

    /// Before focusing workspace `target` on komorebi monitor `km` (bar clicks).
    pub(super) fn ws_anim_before_focus(&self, state: &KState, km: usize, target: usize) {
        if state.monitors.get(km).is_some_and(|m| m.focused != target && target < m.workspaces.len()) {
            self.ws_anim_begin(state, km, Some(target));
        }
    }

    /// komorebi reported a new state.
    pub(super) fn ws_anim_on_state(&self, old: &KState, new: &KState) {
        for (km, m) in new.monitors.iter().enumerate() {
            let Some(prev) = old.monitors.get(km) else { continue };
            if prev.focused == m.focused || (prev.left, prev.top) != (m.left, m.top) {
                continue;
            }
            let staged = self.st.borrow().ws_anim.as_ref().filter(|t| t.monitor == km).map(|t| (t.to, t.cancel.is_some(), t.played));
            log::debug!("workspace anim: monitor {km} switched {} -> {} (staged: {staged:?})", prev.focused, m.focused);
            match staged {
                // The switch the slide anticipated: done once the slide is.
                Some((Some(to), true, played)) if to == m.focused => {
                    if played {
                        self.ws_anim_stop();
                    } else if let Some(t) = self.st.borrow_mut().ws_anim.as_mut() {
                        t.switched = true;
                    }
                }
                // Covered, waiting to learn what comes in.
                Some((None, false, _)) => self.ws_anim_play(new, km, m.focused, true),
                // Something else happened (or nothing was staged): play it now.
                _ => {
                    if self.ws_anim_cover(old, km) {
                        self.ws_anim_play(new, km, m.focused, true);
                    }
                }
            }
            return; // one monitor at a time
        }
    }

    /// Ends any transition now (the real windows are in place underneath).
    pub(super) fn ws_anim_stop(&self) {
        let ended = self.st.borrow_mut().ws_anim.take();
        if let Some(cancel) = ended.as_ref().and_then(|t| t.cancel.as_ref()) {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(stage) = self.ws_stage.borrow_mut().as_mut() {
            stage.release();
        }
        if ended.is_some() {
            // Arriving at a full-screen video hides the bar at once.
            self.refresh_fullscreen();
        }
    }

    /// Covers monitor `km` and, when the destination is known, starts sliding.
    fn ws_anim_begin(&self, state: &KState, km: usize, target: Option<usize>) {
        if !self.ws_anim_cover(state, km) {
            return;
        }
        match target {
            Some(to) => self.ws_anim_play(state, km, to, false),
            None => self.ws_anim_expire(SWITCH_TIMEOUT),
        }
    }

    /// Puts the stage over komorebi monitor `km`, mirroring its focused workspace.
    fn ws_anim_cover(&self, state: &KState, km: usize) -> bool {
        if self.ws_anim_settings().is_none() || self.ws_anim_in_the_way() {
            return false;
        }
        self.ws_anim_stop(); // a running transition jumps to its end
        let Some(m) = state.monitors.get(km) else { return false };
        let Some(windows) = m.workspaces.get(m.focused).map(|w| w.windows.clone()) else { return false };
        let Some(area) = self.ws_anim_area(state, km) else { return false };
        // The bar and frame stay above the stage, like Hyprland's layers;
        // Settings (not topmost, not tiled) is mirrored in place.
        let keep_above: Vec<HWND> = self.screens.borrow().iter().flat_map(|s| [win::hwnd_of(s.border.window()), win::hwnd_of(s.bar.window())]).flatten().collect();
        let pinned: Vec<isize> = if self.st.borrow().settings_visible { win::hwnd_of(self.settings.window()).map(|h| h.0 as isize).into_iter().collect() } else { Vec::new() };
        {
            let mut stage = self.ws_stage.borrow_mut();
            if stage.is_none() {
                *stage = Overlay::new();
            }
            let Some(stage) = stage.as_mut() else { return false };
            if !stage.cover(area, &windows, &pinned, &keep_above) {
                log::warn!("workspace anim: could not mirror the desktop");
                return false;
            }
        }
        let mut st = self.st.borrow_mut();
        st.ws_anim_gen += 1;
        let generation = st.ws_anim_gen;
        st.ws_anim = Some(WsTransition { monitor: km, from: m.focused, to: None, generation, cancel: None, switched: false, played: false });
        true
    }

    /// Slides from the covered workspace to workspace `to` of `state`.
    fn ws_anim_play(&self, state: &KState, km: usize, to: usize, switched: bool) {
        let Some((style, duration)) = self.ws_anim_settings() else { return self.ws_anim_stop() };
        let Some((from, generation)) = self.st.borrow().ws_anim.as_ref().map(|t| (t.from, t.generation)) else { return };
        let incoming = state.monitors.get(km).and_then(|m| m.workspaces.get(to)).map(|w| w.windows.clone()).unwrap_or_default();
        let forward = to > from;
        let (sprites, size) = {
            let mut stage = self.ws_stage.borrow_mut();
            let Some(stage) = stage.as_mut() else { return };
            stage.add_incoming(&incoming);
            (stage.sprites(), stage.size())
        };
        log::debug!("workspace anim: {from} -> {to} on monitor {km}, {} sprites, {duration:?}", sprites.len());
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(t) = self.st.borrow_mut().ws_anim.as_mut() {
            t.to = Some(to);
            t.switched = switched;
            t.cancel = Some(cancel.clone());
        }
        // Frames are paced by DWM (one per composition) off the UI thread.
        let spawned = std::thread::Builder::new().name("workspace-anim".into()).spawn(move || {
            let start = Instant::now();
            loop {
                let t = start.elapsed().as_secs_f64() / duration.as_secs_f64();
                let done = t >= 1.0 || cancel.load(Ordering::Relaxed);
                let f = wsa::frame(style, forward, size, if done { 1.0 } else { wsa::cubic_bezier(wsa::STANDARD, t) });
                for s in &sprites {
                    match s.layer {
                        Layer::Old => s.place(f.old, f.old_alpha),
                        Layer::New => s.place(f.new, f.new_alpha),
                        Layer::Pinned => {}
                    }
                }
                stage::wait_for_compositor();
                if done {
                    break;
                }
            }
            post(move |app| app.ws_anim_played(generation));
        });
        if spawned.is_err() {
            self.ws_anim_stop();
        }
    }

    /// The slide reached its end: uncover now, or once komorebi has switched.
    fn ws_anim_played(&self, generation: u64) {
        let switched = {
            let mut st = self.st.borrow_mut();
            let Some(t) = st.ws_anim.as_mut().filter(|t| t.generation == generation) else { return };
            t.played = true;
            t.switched
        };
        if switched {
            self.ws_anim_stop();
        } else {
            self.ws_anim_expire(SWITCH_TIMEOUT);
        }
    }

    /// Uncovers after `after` if this transition is still waiting for komorebi.
    fn ws_anim_expire(&self, after: Duration) {
        let Some(generation) = self.st.borrow().ws_anim.as_ref().map(|t| t.generation) else { return };
        let weak = self.weak.clone();
        Timer::single_shot(after, move || {
            let Some(app) = weak.upgrade() else { return };
            if app.st.borrow().ws_anim.as_ref().is_some_and(|t| t.generation == generation && !t.switched) {
                log::debug!("workspace anim: komorebi did not switch, uncovering");
                app.ws_anim_stop();
            }
        });
    }

    /// The screen area of komorebi monitor `km` (where its windows live).
    fn ws_anim_area(&self, state: &KState, km: usize) -> Option<Rect> {
        let screens = self.screens.borrow();
        screens.iter().enumerate().find(|(i, s)| self.komorebi_monitor_for_rect(&s.monitor, state, *i) == Some(km)).map(|(_, s)| s.area)
    }

    /// Open drawers and popouts float over the windows; animating under them
    /// would hide them for the length of the slide.
    fn ws_anim_in_the_way(&self) -> bool {
        let st = self.st.borrow();
        st.popout.open || st.drawers.iter().any(|d| d.open)
    }
}
