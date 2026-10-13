//! Built-in static colour schemes (Caelestia ships catppuccin, gruvbox,
//! rosepine, ... next to the dynamic one). Each preset is a compact palette
//! that is expanded into the full set of Material 3 roles, written to
//! `scheme.json` with its `name`/`flavour` exactly like `caelestia scheme set`.

use crate::theme::Scheme;

#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub name: &'static str,
    pub flavour: &'static str,
    pub label: &'static str,
    pub dark: bool,
    pub base: u32,
    pub mantle: u32,
    pub surface0: u32,
    pub surface1: u32,
    pub surface2: u32,
    pub text: u32,
    pub subtext: u32,
    pub overlay: u32,
    pub primary: u32,
    pub secondary: u32,
    pub tertiary: u32,
    pub error: u32,
    pub success: u32,
}

macro_rules! preset {
    ($name:literal, $flavour:literal, $label:literal, $dark:literal, [$base:literal, $mantle:literal, $s0:literal, $s1:literal, $s2:literal], [$text:literal, $sub:literal, $ov:literal], [$p:literal, $s:literal, $t:literal, $e:literal, $ok:literal]) => {
        Preset {
            name: $name, flavour: $flavour, label: $label, dark: $dark,
            base: $base, mantle: $mantle, surface0: $s0, surface1: $s1, surface2: $s2,
            text: $text, subtext: $sub, overlay: $ov,
            primary: $p, secondary: $s, tertiary: $t, error: $e, success: $ok,
        }
    };
}

pub const PRESETS: &[Preset] = &[
    preset!("catppuccin", "mocha", "Catppuccin Mocha", true, [0x1e1e2e, 0x181825, 0x313244, 0x45475a, 0x585b70], [0xcdd6f4, 0xbac2de, 0x7f849c], [0xcba6f7, 0x89b4fa, 0xf5c2e7, 0xf38ba8, 0xa6e3a1]),
    preset!("catppuccin", "macchiato", "Catppuccin Macchiato", true, [0x24273a, 0x1e2030, 0x363a4f, 0x494d64, 0x5b6078], [0xcad3f5, 0xb8c0e0, 0x8087a2], [0xc6a0f6, 0x8aadf4, 0xf5bde6, 0xed8796, 0xa6da95]),
    preset!("catppuccin", "frappe", "Catppuccin Frappé", true, [0x303446, 0x292c3c, 0x414559, 0x51576d, 0x626880], [0xc6d0f5, 0xb5bfe2, 0x838ba7], [0xca9ee6, 0x8caaee, 0xf4b8e4, 0xe78284, 0xa6d189]),
    preset!("catppuccin", "latte", "Catppuccin Latte", false, [0xeff1f5, 0xe6e9ef, 0xccd0da, 0xbcc0cc, 0xacb0be], [0x4c4f69, 0x5c5f77, 0x8c8fa1], [0x8839ef, 0x1e66f5, 0xea76cb, 0xd20f39, 0x40a02b]),
    preset!("rosepine", "main", "Rosé Pine", true, [0x191724, 0x1f1d2e, 0x26233a, 0x2a2837, 0x403d52], [0xe0def4, 0x908caa, 0x6e6a86], [0xc4a7e7, 0x9ccfd8, 0xebbcba, 0xeb6f92, 0x31748f]),
    preset!("rosepine", "moon", "Rosé Pine Moon", true, [0x232136, 0x2a273f, 0x393552, 0x44415a, 0x56526e], [0xe0def4, 0x908caa, 0x6e6a86], [0xc4a7e7, 0x9ccfd8, 0xea9a97, 0xeb6f92, 0x3e8fb0]),
    preset!("rosepine", "dawn", "Rosé Pine Dawn", false, [0xfaf4ed, 0xfffaf3, 0xf2e9e1, 0xdfdad9, 0xcecacd], [0x575279, 0x797593, 0x9893a5], [0x907aa9, 0x56949f, 0xd7827e, 0xb4637a, 0x286983]),
    preset!("gruvbox", "hard", "Gruvbox Dark", true, [0x1d2021, 0x282828, 0x3c3836, 0x504945, 0x665c54], [0xebdbb2, 0xd5c4a1, 0xa89984], [0xfabd2f, 0x83a598, 0xd3869b, 0xfb4934, 0xb8bb26]),
    preset!("gruvbox", "light", "Gruvbox Light", false, [0xfbf1c7, 0xf2e5bc, 0xebdbb2, 0xd5c4a1, 0xbdae93], [0x3c3836, 0x504945, 0x7c6f64], [0xb57614, 0x076678, 0x8f3f71, 0x9d0006, 0x79740e]),
    preset!("nord", "default", "Nord", true, [0x2e3440, 0x272c36, 0x3b4252, 0x434c5e, 0x4c566a], [0xeceff4, 0xd8dee9, 0x8a93a6], [0x88c0d0, 0x81a1c1, 0xb48ead, 0xbf616a, 0xa3be8c]),
    preset!("tokyonight", "night", "Tokyo Night", true, [0x1a1b26, 0x16161e, 0x24283b, 0x292e42, 0x3b4261], [0xc0caf5, 0xa9b1d6, 0x565f89], [0x7aa2f7, 0x7dcfff, 0xbb9af7, 0xf7768e, 0x9ece6a]),
    preset!("onedark", "default", "One Dark", true, [0x282c34, 0x21252b, 0x2c313a, 0x3e4451, 0x4b5263], [0xabb2bf, 0x9da5b4, 0x5c6370], [0x61afef, 0xc678dd, 0x56b6c2, 0xe06c75, 0x98c379]),
    preset!("everforest", "dark", "Everforest", true, [0x2d353b, 0x232a2e, 0x343f44, 0x3d484d, 0x475258], [0xd3c6aa, 0x9da9a0, 0x7a8478], [0xa7c080, 0x7fbbb3, 0xd699b6, 0xe67e80, 0x83c092]),
    preset!("dracula", "default", "Dracula", true, [0x282a36, 0x21222c, 0x343746, 0x44475a, 0x565970], [0xf8f8f2, 0xd6d6d0, 0x6272a4], [0xbd93f9, 0x8be9fd, 0xff79c6, 0xff5555, 0x50fa7b]),
    preset!("kanagawa", "wave", "Kanagawa", true, [0x1f1f28, 0x16161d, 0x2a2a37, 0x363646, 0x54546d], [0xdcd7ba, 0xc8c093, 0x727169], [0x7e9cd8, 0x957fb8, 0xd27e99, 0xe82424, 0x98bb6c]),
];

pub fn find(name: &str, flavour: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name && p.flavour == flavour).or_else(|| PRESETS.iter().find(|p| p.name == name))
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 0xff) as f32;
    let m = |s: u32| ((ch(a, s) * (1.0 - t) + ch(b, s) * t).round() as u32) << s;
    m(16) | m(8) | m(0)
}

/// Expands a preset into a Caelestia-format scheme.
pub fn to_scheme(p: &Preset) -> Scheme {
    let on = |c: u32| if p.dark { mix(c, 0x000000, 0.72) } else { 0xffffff };
    let container = |c: u32| if p.dark { mix(c, p.base, 0.7) } else { mix(c, 0xffffff, 0.75) };
    let on_container = |c: u32| if p.dark { mix(c, 0xffffff, 0.55) } else { mix(c, 0x000000, 0.55) };
    let roles: [(&str, u32); 37] = [
        ("background", p.base),
        ("onBackground", p.text),
        ("surface", p.base),
        ("surfaceDim", if p.dark { p.mantle } else { p.surface0 }),
        ("surfaceBright", p.surface1),
        ("surfaceContainerLowest", if p.dark { mix(p.mantle, 0x000000, 0.25) } else { 0xffffff }),
        ("surfaceContainerLow", p.mantle),
        ("surfaceContainer", mix(p.base, p.surface0, 0.5)),
        ("surfaceContainerHigh", p.surface0),
        ("surfaceContainerHighest", p.surface1),
        ("onSurface", p.text),
        ("surfaceVariant", p.surface1),
        ("onSurfaceVariant", p.subtext),
        ("inverseSurface", p.text),
        ("inverseOnSurface", p.base),
        ("outline", p.overlay),
        ("outlineVariant", p.surface2),
        ("shadow", 0x000000),
        ("scrim", 0x000000),
        ("surfaceTint", p.primary),
        ("primary", p.primary),
        ("onPrimary", on(p.primary)),
        ("primaryContainer", container(p.primary)),
        ("onPrimaryContainer", on_container(p.primary)),
        ("inversePrimary", mix(p.primary, p.base, 0.4)),
        ("secondary", p.secondary),
        ("onSecondary", on(p.secondary)),
        ("secondaryContainer", container(p.secondary)),
        ("onSecondaryContainer", on_container(p.secondary)),
        ("tertiary", p.tertiary),
        ("onTertiary", on(p.tertiary)),
        ("tertiaryContainer", container(p.tertiary)),
        ("onTertiaryContainer", on_container(p.tertiary)),
        ("error", p.error),
        ("onError", on(p.error)),
        ("errorContainer", container(p.error)),
        ("onErrorContainer", on_container(p.error)),
    ];
    let mut s = Scheme::generate(material_colors::color::Rgb::from_u32(p.primary), "tonalspot", p.dark);
    s.name = p.name.into();
    s.flavour = p.flavour.into();
    s.variant = "static".into();
    for (k, v) in roles {
        s.colours.insert(k.into(), format!("{v:06x}"));
    }
    s.colours.insert("success".into(), format!("{:06x}", p.success));
    s
}
