//! "Confetti" in the launcher: two cannons in the bottom corners fire a
//! burst towards the middle of the screen. This generates the pieces; the
//! motion itself (air drag, flutter, tumbling, fade) is a closed-form
//! function of time in `ui/modules/confetti.slint`.

/// Seconds the overlay stays up (longest delay + life, plus a margin).
pub const DURATION_S: f32 = 5.0;
/// Air drag of the UI's motion model (1/s): launch speed fades this fast.
pub const DRAG: f32 = 2.0;
const PER_CANNON: usize = 100;

/// One piece of confetti, in logical pixels and seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Piece {
    /// Launch point.
    pub x: f32,
    pub y: f32,
    /// Launch velocity per second (y grows downwards).
    pub vx: f32,
    pub vy: f32,
    pub w: f32,
    pub h: f32,
    /// A dot instead of a strip.
    pub round: bool,
    /// Index into the caller's colours.
    pub colour: usize,
    /// Degrees per second.
    pub spin: f32,
    /// Tumbles per second (the strip flips edge-on and back).
    pub flip: f32,
    /// Side-to-side flutter while falling.
    pub sway: f32,
    /// Degrees: start angle and flutter phase.
    pub phase: f32,
    pub delay: f32,
    pub life: f32,
}

/// xorshift64*: deterministic, good enough for confetti.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// A burst for a `width` x `height` screen with `colours` colours.
pub fn burst(width: f32, height: f32, colours: usize, seed: u64) -> Vec<Piece> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(PER_CANNON * 2);
    for left in [true, false] {
        let dir = if left { 1.0 } else { -1.0 };
        for i in 0..PER_CANNON {
            // With drag k, a launch speed v travels v/k: aim for a rise of
            // 55-95% of the screen and a reach of 5-45% of its width.
            let rise = height * rng.range(0.55, 0.95);
            let reach = width * rng.range(0.05, 0.45);
            // Two waves, like a party popper: most at once, a few right after.
            let delay = if i % 10 < 7 { rng.range(0.0, 0.15) } else { rng.range(0.35, 0.5) };
            out.push(Piece {
                x: if left { -10.0 } else { width + 10.0 },
                y: height + 10.0,
                vx: dir * reach * DRAG,
                vy: -rise * DRAG,
                w: rng.range(12.0, 22.0),
                h: rng.range(7.0, 11.0),
                round: rng.next() < 0.2,
                colour: (rng.next() * colours.max(1) as f32) as usize % colours.max(1),
                spin: rng.range(-540.0, 540.0),
                flip: rng.range(0.6, 2.2),
                sway: rng.range(10.0, 40.0),
                phase: rng.range(0.0, 360.0),
                delay,
                life: rng.range(3.6, 4.4),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_cannons_fire_up_towards_the_middle() {
        let pieces = burst(2560.0, 1440.0, 5, 42);
        assert_eq!(pieces.len(), 2 * PER_CANNON);
        for p in &pieces {
            assert!(p.y > 1440.0, "starts below the bottom edge");
            assert!(p.vy < 0.0, "goes up");
            let from_left = p.x < 0.0;
            assert!(if from_left { p.vx > 0.0 } else { p.x > 2560.0 && p.vx < 0.0 }, "towards the middle");
            // Peak height (v/k) stays on screen.
            assert!(-p.vy / DRAG <= 1440.0 * 0.95 + 0.01);
            assert!(p.colour < 5);
            assert!(p.delay + p.life < DURATION_S, "gone before the overlay closes");
        }
        assert!(pieces.iter().any(|p| p.round) && pieces.iter().any(|p| !p.round));
    }

    #[test]
    fn same_seed_same_burst() {
        assert_eq!(burst(800.0, 600.0, 3, 7), burst(800.0, 600.0, 3, 7));
        assert_ne!(burst(800.0, 600.0, 3, 7), burst(800.0, 600.0, 3, 8));
    }
}
