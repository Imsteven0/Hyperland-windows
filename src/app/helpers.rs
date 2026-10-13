//! Free helpers shared by the shell controller.

use super::*;

// ----------------------------------------------------------------------
// helpers
// ----------------------------------------------------------------------

/// Switches the bar's active workspace, telling it the direction first so
/// the active trail stretches the right way.
pub(super) fn set_active_workspace(g: &ShellGlobal<'_>, active: i32) {
    let previous = g.get_active_workspace();
    if previous != active {
        g.set_workspace_direction(workspace_direction(previous, active));
        g.set_active_workspace(active);
    }
}

pub(super) fn workspace_direction(from: i32, to: i32) -> i32 {
    if to < from { -1 } else { 1 }
}

/// Long bodies are cut to a few lines so one chatty toast can't fill the sidebar.
pub(super) fn notification_body(body: &str) -> String {
    const MAX_LINES: usize = 4;
    const MAX_CHARS: usize = 280;
    let lines: Vec<&str> = body.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let mut out = lines.iter().take(MAX_LINES).copied().collect::<Vec<_>>().join("\n");
    let cut = lines.len() > MAX_LINES || out.chars().count() > MAX_CHARS;
    if out.chars().count() > MAX_CHARS {
        out = out.chars().take(MAX_CHARS).collect::<String>().trim_end().to_owned();
    }
    if cut {
        out.push('…');
    }
    out
}

pub(super) fn is_integral_key(path: &str) -> bool {
    matches!(path, "border.thickness" | "border.rounding" | "bar.workspaces.shown" | "launcher.maxShown" | "bar.sizes.innerWidth") || path.starts_with("windows.komorebi.")
}

/// Windows locale, e.g. "es-CR".
pub(super) fn system_locale() -> String {
    let mut buf = [0u16; 85];
    let n = unsafe { windows::Win32::Globalization::GetUserDefaultLocaleName(&mut buf) };
    String::from_utf16_lossy(&buf[..(n.max(1) as usize - 1)])
}

/// Two-letter UI language of Windows (for localised place names).
pub(super) fn system_lang() -> String {
    system_locale().split('-').next().filter(|s| s.len() == 2).unwrap_or("en").to_lowercase()
}

/// Two-letter region of Windows ("CR"), used to rank local places first.
pub(super) fn system_region() -> String {
    system_locale().rsplit('-').next().filter(|s| s.len() == 2).unwrap_or_default().to_uppercase()
}

pub(super) fn weather_query(cfg: &Config) -> WeatherQuery {
    WeatherQuery { lang: crate::i18n::lang(), location: cfg.str("services.weatherLocation"), name: cfg.str("services.weatherLocationName"), fahrenheit: cfg.bool("services.useFahrenheit"), twelve_hour: cfg.bool("services.useTwelveHourClock") }
}

pub(super) fn recordings_dir(cfg: &Config) -> PathBuf {
    let p = cfg.path("paths.recordingsDir");
    if p.as_os_str().is_empty() { dirs::video_dir().unwrap_or_default() } else { p }
}

pub(super) fn current_wallpaper_path(cfg: &Config) -> Option<PathBuf> {
    std::fs::read_to_string(cfg.paths.state_dir.join("wallpaper").join("path.txt"))
        .ok()
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| p.exists())
        .or_else(|| system::current_wallpaper().map(PathBuf::from).filter(|p| p.exists()))
}

pub(super) fn load_or_generate_scheme(cfg: &Config) -> Scheme {
    if let Ok(s) = Scheme::load(&cfg.paths.scheme_json) {
        return s;
    }
    let source = current_wallpaper_path(cfg).and_then(|p| source_from_image(&p).ok()).unwrap_or(material_colors::color::Rgb::from_u32(0x4a6fa5));
    let scheme = Scheme::generate(source, "tonalspot", true);
    if let Err(e) = scheme.save(&cfg.paths.scheme_json) {
        log::warn!("could not write scheme.json: {e:#}");
    }
    scheme
}

/// Configured wallpaper folder, or Windows' built-in wallpapers when it's missing.
pub(super) fn wallpaper_dir(cfg: &Config) -> PathBuf {
    let dir = cfg.path("paths.wallpaperDir");
    if dir.is_dir() {
        return dir;
    }
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    PathBuf::from(windir).join("Web").join("Wallpaper")
}

/// Settings grid page size.
pub(super) const WALLPAPER_PAGE: usize = 60;

/// "12 MB" / "1.4 GB" ("1,4 GB" in Spanish).
pub(super) fn size_label(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    let s = if mb >= 1024.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{mb:.0} MB") };
    if crate::i18n::is_es() { s.replace('.', ",") } else { s }
}

pub(super) fn list_wallpapers(cfg: &Config) -> Vec<PathBuf> {
    let root = wallpaper_dir(cfg);
    let max = 10_000;
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if matches!(p.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("jpg" | "jpeg" | "png" | "webp" | "bmp")) {
                out.push(p);
                if out.len() >= max {
                    break;
                }
            }
        }
    }
    out.sort();
    out
}

pub(super) fn average_luminance(path: &Path) -> Option<f32> {
    let img = image::open(path).ok()?.thumbnail(32, 32).into_rgb8();
    let n = (img.width() * img.height()) as f32;
    Some(img.pixels().map(|p| (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0).sum::<f32>() / n)
}

pub(super) fn add_months(d: NaiveDate, months: i32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + months;
    NaiveDate::from_ymd_opt(total.div_euclid(12), total.rem_euclid(12) as u32 + 1, 1).unwrap_or(d)
}

/// 6×7 grid for the month containing `month`, highlighting `today`.
/// Parses a number Slint formatted as text: float-to-string follows the UI
/// locale, so Spanish sends "0,45".
pub(super) fn parse_num<T: std::str::FromStr>(s: &str) -> Option<T> {
    s.trim().replace(',', ".").parse().ok()
}

pub(super) fn calendar(month: NaiveDate, today: NaiveDate) -> (String, Vec<CalendarDay>) {
    let first = month.with_day(1).unwrap_or(month);
    let offset = first.weekday().num_days_from_monday() as i64;
    let start = first - chrono::Duration::days(offset);
    let days = (0..42)
        .map(|i| {
            let d = start + chrono::Duration::days(i);
            CalendarDay { day: d.day() as i32, current_month: d.month() == first.month(), today: d == today }
        })
        .collect();
    (crate::i18n::capitalize(&first.format_localized("%B %Y", crate::i18n::chrono_locale()).to_string()), days)
}

/// Material 3 expressive "9-sided cookie" outline in a 100x100 viewbox.
pub(super) fn cookie_path() -> String {
    let (n, lobes) = (180, 9.0f64);
    let mut s = String::new();
    for i in 0..=n {
        let a = i as f64 / n as f64 * std::f64::consts::TAU;
        let r = 46.0 * (0.9 + 0.1 * (lobes * a).cos());
        let (x, y) = (50.0 + r * a.cos(), 50.0 + r * a.sin());
        s.push_str(&format!("{}{x:.2} {y:.2}", if i == 0 { "M " } else { " L " }));
    }
    s.push_str(" Z");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_bodies_are_capped() {
        assert_eq!(notification_body("  hola \n\n que tal "), "hola\nque tal");
        assert_eq!(notification_body("1\n2\n3\n4\n5"), "1\n2\n3\n4…");
        let long = "x".repeat(400);
        assert_eq!(notification_body(&long).chars().count(), 281);
        assert_eq!(notification_body(""), "");
    }

    #[test]
    fn workspace_direction_follows_the_switch() {
        assert_eq!(workspace_direction(1, 3), 1);
        assert_eq!(workspace_direction(4, 2), -1);
        assert_eq!(workspace_direction(2, 2), 1);
    }

    #[test]
    fn parses_locale_numbers() {
        // Slint formats floats with the UI locale: Spanish sends "0,45".
        assert_eq!(parse_num::<f64>("0,45"), Some(0.45));
        assert_eq!(parse_num::<f64>(" 0.45 "), Some(0.45));
        assert_eq!(parse_num::<f32>("12"), Some(12.0));
        assert_eq!(parse_num::<f64>("abc"), None);
    }

    #[test]
    fn months_wrap_across_years() {
        let d = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        assert_eq!(add_months(d, -1), NaiveDate::from_ymd_opt(2025, 12, 1).unwrap());
        assert_eq!(add_months(d, 12), NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
        assert_eq!(add_months(d, 0), NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
    }

    #[test]
    fn integral_keys_stay_integers() {
        assert!(is_integral_key("border.thickness"));
        assert!(is_integral_key("windows.komorebi.borderWidth"));
        assert!(!is_integral_key("appearance.transparency.base"));
    }
}
