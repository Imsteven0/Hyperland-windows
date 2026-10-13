//! Performance mode: decides when to drop blur, animations and edge hover.

use super::*;

impl Caelestia {
    pub(super) fn perf_reason(&self) -> Option<PerfReason> {
        self.st.borrow().perf.state()
    }

    /// Animation speed, or (almost) instant while in performance mode.
    pub(super) fn anim_scale(&self) -> f32 {
        if self.perf_reason().is_some() { 0.0 } else { self.cfg.borrow().f32("appearance.anim.durations.scale") }
    }

    /// Re-evaluates performance mode (every 2 s and on config changes).
    pub(super) fn refresh_perf(&self) {
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
            // Name the app for full screen: a false positive is then easy to spot.
            let app = if state == Some(PerfReason::Fullscreen) { desktops::window_info(win::foreground_window()).map(|i| format!(" ({})", i.app)) } else { None };
            log::info!("performance mode: {}{}", state.map(PerfReason::id).unwrap_or("off"), app.unwrap_or_default());
            self.apply_config(); // also re-applies the bar blur
            if state.is_some() {
                self.update_blur(); // hides the panes now; the loop stops itself
            }
        }
    }
}
