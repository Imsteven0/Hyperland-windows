//! "Confetti" (launcher action, `caelestia confetti`): a burst over the
//! screen under the pointer, in the colour scheme, without taking clicks.

use super::*;
use crate::{ConfettiPiece, ConfettiWindow, services::confetti};

impl Caelestia {
    pub(super) fn play_confetti(&self) {
        let (area, scale) = self.screen_geom(self.active_screen());
        let p = self.scheme.borrow().palette();
        // The scheme's bright tones (containers are dark in dark schemes).
        let colours = [p.primary, p.secondary, p.tertiary, p.error, p.on_primary_container, p.on_secondary_container];
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        let pieces: Vec<ConfettiPiece> = confetti::burst(area.width() as f32 / scale, area.height() as f32 / scale, colours.len(), seed)
            .into_iter()
            .map(|q| ConfettiPiece {
                x: q.x,
                y: q.y,
                vx: q.vx,
                vy: q.vy,
                w: q.w,
                h: q.h,
                round: q.round,
                tint: colours[q.colour],
                spin: q.spin,
                flip: q.flip,
                sway: q.sway,
                phase: q.phase,
                delay: q.delay,
                life: q.life,
            })
            .collect();
        {
            let mut slot = self.confetti.borrow_mut();
            if slot.is_none() {
                let Ok(w) = ConfettiWindow::new() else { return };
                let weak = self.weak.clone();
                w.on_done(move || {
                    if let Some(app) = weak.upgrade()
                        && let Some(w) = app.confetti.borrow().as_ref()
                    {
                        let _ = w.hide();
                    }
                });
                *slot = Some(w);
            }
            let Some(w) = slot.as_ref() else { return };
            w.set_seconds(confetti::DURATION_S);
            w.set_drag(confetti::DRAG);
            w.set_pieces(ModelRc::new(VecModel::from(pieces)));
            w.window().set_position(PhysicalPosition::new(area.left, area.top));
            w.window().set_size(slint::PhysicalSize::new(area.width() as u32, area.height() as u32));
            if w.show().is_err() {
                return;
            }
        }
        self.confetti_start(0);
    }

    /// winit creates the native window asynchronously: make it click-through
    /// once it exists, then fire.
    fn confetti_start(&self, tries: u32) {
        let hwnd = self.confetti.borrow().as_ref().and_then(|w| win::hwnd_of(w.window()));
        match hwnd {
            Some(h) => {
                win::make_surface(h, SurfaceStyle { click_through: true, no_activate: true, topmost: true });
                if let Some(w) = self.confetti.borrow().as_ref() {
                    w.invoke_play();
                }
            }
            None if tries < 30 => {
                let weak = self.weak.clone();
                Timer::single_shot(Duration::from_millis(16), move || {
                    if let Some(app) = weak.upgrade() {
                        app.confetti_start(tries + 1);
                    }
                });
            }
            None => log::warn!("confetti: no window"),
        }
    }
}
