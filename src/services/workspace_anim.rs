//! Workspace switch animation, as in Caelestia's Hyprland config:
//! `workspaces, speed 5, bezier standard` with the default `slide` style.
//! komorebi switches workspaces instantly (it cloaks one set of windows and
//! uncloaks the other), so the shell plays the transition on an overlay with
//! live DWM thumbnails (see `platform::workspace_overlay`). This module is the
//! platform-free part: timing, easing, per-frame geometry and which komorebic
//! commands switch workspaces.

use std::time::Duration;

/// Hyprland's `speed 5` (speed is in tenths of a second).
pub const BASE_DURATION_MS: f32 = 500.0;

/// Caelestia's `windowsMove`: speed 6 on the same `standard` curve, used for
/// komorebi's movement animations.
pub const MOVE_DURATION_MS: f32 = 600.0;

/// Material 3 "standard" easing, Caelestia's `standard` bezier.
pub const STANDARD: [f64; 4] = [0.2, 0.0, 0.0, 1.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Hyprland's default: the whole workspace slides horizontally.
    Slide,
    /// `slidevert`: vertically, matching the bar's workspace column.
    SlideVert,
    /// Cross-fade in place.
    Fade,
}

impl Style {
    /// `windows.komorebi.workspaceAnimation`; `None` turns the animation off.
    pub fn parse(s: &str) -> Option<Style> {
        match s.trim().to_ascii_lowercase().as_str() {
            "slide" | "" => Some(Style::Slide),
            "slidevert" | "slide-vert" | "vertical" => Some(Style::SlideVert),
            "fade" => Some(Style::Fade),
            _ => None, // "none", "off"…
        }
    }
}

/// Animation length for the shell's speed scale (0 = performance mode: none).
pub fn duration(scale: f32) -> Option<Duration> {
    let ms = BASE_DURATION_MS * scale;
    (ms >= 1.0).then(|| Duration::from_millis(ms.round() as u64))
}

/// CSS-style `cubic-bezier(x1, y1, x2, y2)` evaluated at `t` in 0..=1.
pub fn cubic_bezier([x1, y1, x2, y2]: [f64; 4], t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let coord = |a: f64, b: f64, s: f64| 3.0 * a * s * (1.0 - s).powi(2) + 3.0 * b * s * s * (1.0 - s) + s.powi(3);
    let slope = |a: f64, b: f64, s: f64| 3.0 * a * (1.0 - s).powi(2) + 6.0 * (b - a) * s * (1.0 - s) + 3.0 * (1.0 - b) * s * s;
    // Solve x(s) = t: Newton first, bisection when the slope is too flat.
    let mut s = t;
    for _ in 0..8 {
        let dx = slope(x1, x2, s);
        if dx.abs() < 1e-6 {
            break;
        }
        let next = s - (coord(x1, x2, s) - t) / dx;
        if !(0.0..=1.0).contains(&next) {
            break;
        }
        s = next;
    }
    if (coord(x1, x2, s) - t).abs() > 1e-5 {
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..40 {
            s = (lo + hi) / 2.0;
            if coord(x1, x2, s) < t { lo = s } else { hi = s }
        }
    }
    coord(y1, y2, s)
}

/// Where the two workspaces are drawn at eased progress `p` (0..=1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// Offset of the workspace being left, in pixels.
    pub old: (i32, i32),
    /// Offset of the workspace being entered.
    pub new: (i32, i32),
    pub old_alpha: u8,
    pub new_alpha: u8,
}

/// `forward` = going to a later workspace: like Hyprland, it comes in from
/// the right (or from below) while the current one leaves the other way.
pub fn frame(style: Style, forward: bool, (w, h): (i32, i32), p: f64) -> Frame {
    let p = p.clamp(0.0, 1.0);
    let sign = if forward { 1.0 } else { -1.0 };
    let shift = |extent: i32| ((extent as f64) * p * sign).round() as i32;
    let enter = |extent: i32| ((extent as f64) * (1.0 - p) * sign).round() as i32;
    match style {
        Style::Slide => Frame { old: (-shift(w), 0), new: (enter(w), 0), old_alpha: 255, new_alpha: 255 },
        Style::SlideVert => Frame { old: (0, -shift(h)), new: (0, enter(h)), old_alpha: 255, new_alpha: 255 },
        Style::Fade => Frame { old: (0, 0), new: (0, 0), old_alpha: (255.0 * (1.0 - p)).round() as u8, new_alpha: (255.0 * p).round() as u8 },
    }
}

/// What a komorebic command does to the focused monitor's workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    /// Shows this workspace (0-based) as it is: the slide can start at once.
    Focus(usize),
    /// Next (+1) or previous (-1) workspace, wrapping around.
    Cycle(i32),
    /// Switches, but what arrives is only known once komorebi is done
    /// (a window moved along, the last workspace…).
    Other,
}

impl Switch {
    /// The workspace shown afterwards, when it can be known beforehand.
    pub fn target(self, focused: usize, count: usize) -> Option<usize> {
        match self {
            Switch::Focus(n) => Some(n),
            Switch::Cycle(step) if count > 0 => Some((focused as i64 + step as i64).rem_euclid(count as i64) as usize),
            _ => None,
        }
    }
}

/// Recognises the komorebic commands (as in `windows.komorebi.binds`) that
/// change the focused monitor's workspace, so the overlay can be up before
/// komorebi hides the windows. Sending a window away without following it
/// (`send-to-workspace`) is not a switch.
pub fn switch_of(command: &str) -> Option<Switch> {
    let mut parts = command.split_whitespace();
    let verb = parts.next()?;
    let arg = parts.next();
    match verb {
        "focus-workspace" | "focus-workspaces" => arg.and_then(|n| n.parse().ok()).map(Switch::Focus),
        "cycle-workspace" => match arg {
            Some("next") => Some(Switch::Cycle(1)),
            Some("previous") => Some(Switch::Cycle(-1)),
            _ => Some(Switch::Other),
        },
        "move-to-workspace" => arg.and_then(|n| n.parse::<usize>().ok()).map(|_| Switch::Other),
        "focus-named-workspace" | "move-to-named-workspace" | "focus-last-workspace" | "cycle-move-to-workspace" => Some(Switch::Other),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_curve_matches_known_points() {
        assert_eq!(cubic_bezier(STANDARD, 0.0), 0.0);
        assert_eq!(cubic_bezier(STANDARD, 1.0), 1.0);
        // Material "standard" decelerates: well past halfway at half time.
        let mid = cubic_bezier(STANDARD, 0.5);
        assert!((0.85..0.92).contains(&mid), "{mid}");
        // Monotonic.
        let mut last = 0.0;
        for i in 1..=100 {
            let v = cubic_bezier(STANDARD, i as f64 / 100.0);
            assert!(v >= last - 1e-9, "not monotonic at {i}");
            last = v;
        }
        // Linear control points give a straight line.
        assert!((cubic_bezier([1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0], 0.3) - 0.3).abs() < 1e-4);
    }

    #[test]
    fn slide_moves_both_workspaces_one_screen() {
        let size = (2560, 1440);
        let start = frame(Style::Slide, true, size, 0.0);
        assert_eq!((start.old, start.new), ((0, 0), (2560, 0)));
        let end = frame(Style::Slide, true, size, 1.0);
        assert_eq!((end.old, end.new), ((-2560, 0), (0, 0)));
        // Going back: the previous workspace comes in from the left.
        let back = frame(Style::Slide, false, size, 0.0);
        assert_eq!(back.new, (-2560, 0));
        let half = frame(Style::Slide, true, size, 0.5);
        assert_eq!(half.new.0 - half.old.0, 2560, "workspaces stay glued together");
    }

    #[test]
    fn vertical_and_fade() {
        let v = frame(Style::SlideVert, true, (2560, 1440), 0.25);
        assert_eq!((v.old, v.new), ((0, -360), (0, 1080)));
        let f = frame(Style::Fade, true, (100, 100), 0.5);
        assert_eq!((f.old, f.new, f.old_alpha, f.new_alpha), ((0, 0), (0, 0), 128, 128));
    }

    #[test]
    fn parses_styles_and_duration() {
        assert_eq!(Style::parse("slide"), Some(Style::Slide));
        assert_eq!(Style::parse(" SlideVert "), Some(Style::SlideVert));
        assert_eq!(Style::parse("fade"), Some(Style::Fade));
        assert_eq!(Style::parse("none"), None);
        assert_eq!(duration(1.0), Some(Duration::from_millis(500)));
        assert_eq!(duration(0.5), Some(Duration::from_millis(250)));
        assert_eq!(duration(0.0), None);
    }

    #[test]
    fn recognises_workspace_switches() {
        assert_eq!(switch_of("focus-workspace 2"), Some(Switch::Focus(2)));
        // The moved window only lands there once komorebi is done.
        assert_eq!(switch_of("move-to-workspace 0"), Some(Switch::Other));
        assert_eq!(switch_of("cycle-workspace previous"), Some(Switch::Cycle(-1)));
        assert_eq!(switch_of("focus-last-workspace"), Some(Switch::Other));
        assert_eq!(switch_of("cycle-move-to-workspace next"), Some(Switch::Other));
        assert_eq!(switch_of("send-to-workspace 3"), None);
        assert_eq!(switch_of("focus left"), None);
        assert_eq!(switch_of("focus-workspace"), None);
    }

    #[test]
    fn targets_known_in_advance() {
        assert_eq!(Switch::Focus(3).target(0, 5), Some(3));
        assert_eq!(Switch::Cycle(1).target(4, 5), Some(0));
        assert_eq!(Switch::Cycle(-1).target(0, 5), Some(4));
        assert_eq!(Switch::Other.target(1, 5), None);
    }
}
