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

/// A value from `Shell.set-config`: JSON, or a number Slint formatted with
/// the UI locale (sliders send "0,85" in Spanish), or else plain text.
pub(super) fn ui_value(json: &str) -> Value {
    serde_json::from_str(json)
        .ok()
        .or_else(|| parse_num::<f64>(json).and_then(serde_json::Number::from_f64).map(Value::Number))
        .unwrap_or_else(|| Value::String(json.to_owned()))
}

/// Integral settings stay integers in the file and the rest is rounded to
/// 3 decimals (slider noise like 0.8500001 makes ugly diffs in shared dots).
pub(super) fn normalize_number(path: &str, value: Value) -> Value {
    match value {
        Value::Number(n) if n.as_f64().is_some_and(|f| f.fract() == 0.0 && is_integral_key(path)) => Value::from(n.as_f64().unwrap_or(0.0) as i64),
        Value::Number(n) => Value::from((n.as_f64().unwrap_or(0.0) * 1000.0).round() / 1000.0),
        v => v,
    }
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
    list_images(&wallpaper_dir(cfg), 10_000)
}

/// Images below `root` (sorted), at most `max`. Uses the type the directory
/// listing already returned instead of one extra stat per entry.
pub(super) fn list_images(root: &Path, max: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    'walk: while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(kind) = e.file_type() else { continue };
            let p = e.path();
            // Symlinks/junctions to folders still count (they need the stat).
            if kind.is_dir() || (kind.is_symlink() && p.is_dir()) {
                stack.push(p);
            } else if matches!(p.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("jpg" | "jpeg" | "png" | "webp" | "bmp")) {
                out.push(p);
                if out.len() >= max {
                    break 'walk;
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

/// Wallpaper preview fitting `px`×`px`: Explorer's thumbnail cache when it has
/// one (fast), else decoded here. Windows without the codec (WebP extension
/// removed by "debloat" scripts, LTSC/N editions) or with thumbnails turned off
/// returns nothing, which left the cards blank. Call off the UI thread.
pub(super) fn wallpaper_thumbnail(path: &Path, px: i32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    path_thumbnail(path, px, false).or_else(|| decode_thumbnail(path, px.max(1) as u32))
}

/// Decodes `path` upright (EXIF orientation) scaled to fit `px`×`px`, as the
/// premultiplied RGBA `Image::from_rgba8_premultiplied` expects.
pub(super) fn decode_thumbnail(path: &Path, px: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.into_decoder().ok()?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orientation);
    let rgba = img.thumbnail(px, px).into_rgba8();
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(rgba.width(), rgba.height());
    for (dst, p) in buf.make_mut_slice().iter_mut().zip(rgba.pixels()) {
        let [r, g, b, a] = p.0;
        let pm = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
        *dst = Rgba8Pixel { r: pm(r), g: pm(g), b: pm(b), a };
    }
    Some(buf)
}

/// One step of Material 3 Expressive's default spatial spring (stiffness 380,
/// damping ratio 0.8: settles in well under 0.5 s with at most a slight
/// overshoot, like the `Motion.spatial` curve). Semi-implicit Euler: stable at
/// frame-sized `dt`.
pub(super) fn spring_step(pos: f32, vel: f32, target: f32, dt: f32) -> (f32, f32) {
    const STIFFNESS: f32 = 380.0;
    const DAMPING: f32 = 2.0 * 0.8 * 19.493_59; // 2 * ratio * sqrt(stiffness)
    let vel = vel + (STIFFNESS * (target - pos) - DAMPING * vel) * dt;
    (pos + vel * dt, vel)
}

/// Makes `model` equal `new` with the fewest row changes, matching rows by
/// `key`: kept rows are updated in place (their UI items survive), the rest
/// are inserted or removed. Keys are assumed unique.
pub(super) fn sync_keyed<T: Clone + 'static, K: PartialEq>(model: &VecModel<T>, new: Vec<T>, key: impl Fn(&T) -> K) {
    use slint::Model;
    for i in (0..model.row_count()).rev() {
        if model.row_data(i).is_some_and(|r| !new.iter().any(|n| key(n) == key(&r))) {
            model.remove(i);
        }
    }
    for (i, item) in new.into_iter().enumerate() {
        let k = key(&item);
        match (i..model.row_count()).find(|&j| model.row_data(j).is_some_and(|r| key(&r) == k)) {
            Some(j) if j == i => model.set_row_data(i, item),
            Some(j) => {
                model.remove(j);
                model.insert(i, item);
            }
            None => model.insert(i, item),
        }
    }
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
    use serde_json::json;

    #[test]
    fn spring_settles_like_the_spatial_curve() {
        // 200 px jump at 60 fps: at most a slight overshoot, settled within 0.6 s.
        let (mut pos, mut vel, mut peak) = (0.0f32, 0.0f32, 0.0f32);
        for _ in 0..36 {
            (pos, vel) = spring_step(pos, vel, 200.0, 1.0 / 60.0);
            peak = peak.max(pos);
        }
        assert!(peak < 206.0, "overshoot {peak}");
        assert!((pos - 200.0).abs() < 0.5 && vel.abs() < 5.0, "settled {pos} {vel}");
    }

    #[test]
    fn keyed_sync_keeps_rows_and_follows_order() {
        use slint::Model;
        let rows = |m: &VecModel<(i32, &'static str)>| (0..m.row_count()).filter_map(|i| m.row_data(i)).collect::<Vec<_>>();
        let m = VecModel::from(vec![(1, "a"), (2, "b"), (3, "c")]);
        // 2 dropped, 3 updated in place, 4 new on top, 1 moved after it.
        sync_keyed(&m, vec![(4, "d"), (3, "C"), (1, "a")], |r| r.0);
        assert_eq!(rows(&m), [(4, "d"), (3, "C"), (1, "a")]);
        sync_keyed(&m, vec![], |r| r.0);
        assert_eq!(m.row_count(), 0);
    }

    #[test]
    fn decoded_thumbnails_fit_and_are_premultiplied() {
        let dir = std::env::temp_dir().join(format!("caelestia-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 400×200 half-transparent red: fits 100 px as 100×50, colour premultiplied.
        let png = dir.join("wall.png");
        image::RgbaImage::from_pixel(400, 200, image::Rgba([200, 0, 0, 128])).save(&png).unwrap();
        let buf = decode_thumbnail(&png, 100).unwrap();
        assert_eq!((buf.width(), buf.height()), (100, 50));
        let p = buf.as_slice()[0];
        assert_eq!((p.r, p.g, p.b, p.a), (100, 0, 0, 128));
        // Not an image: no thumbnail, no panic.
        let txt = dir.join("notes.png");
        std::fs::write(&txt, "not an image").unwrap();
        assert!(decode_thumbnail(&txt, 100).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

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

    /// Spanish sliders send "0,85"; values must stay numbers, never strings.
    #[test]
    fn ui_values_survive_the_decimal_comma() {
        assert_eq!(ui_value("0,85"), json!(0.85));
        assert_eq!(ui_value("0.85"), json!(0.85));
        assert_eq!(ui_value("12"), json!(12));
        assert_eq!(ui_value("-1,5"), json!(-1.5));
        assert_eq!(ui_value("true"), json!(true));
        assert_eq!(ui_value("\"Alt+Space\""), json!("Alt+Space"));
        assert_eq!(ui_value("not json"), json!("not json"));
        // "1,2,3" is neither JSON nor a number: kept as text, never mangled.
        assert_eq!(ui_value("1,2,3"), json!("1,2,3"));
    }

    #[test]
    fn numbers_are_normalized_per_key() {
        assert_eq!(normalize_number("border.thickness", ui_value("12,0")), json!(12));
        assert!(normalize_number("border.thickness", ui_value("12")).is_i64());
        assert_eq!(normalize_number("border.thickness", ui_value("12,5")), json!(12.5), "never truncates a real fraction");
        assert_eq!(normalize_number("appearance.transparency.base", ui_value("0,8500001")), json!(0.85));
        assert_eq!(normalize_number("appearance.transparency.base", ui_value("1")), json!(1.0));
        assert_eq!(normalize_number("general.language", json!("es")), json!("es"));
    }

    #[test]
    fn size_labels_follow_the_locale_decimal() {
        // size_label follows the global UI language: accept either separator.
        assert_eq!(size_label(12 * 1_048_576).replace(',', "."), "12 MB");
        assert_eq!(size_label(1536 * 1_048_576).replace(',', "."), "1.5 GB");
    }

    #[test]
    fn lists_images_recursively_with_a_cap() {
        let root = std::env::temp_dir().join(format!("caelestia-walls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("dark").join("deep")).unwrap();
        for f in ["a.jpg", "b.PNG", "notes.txt", "c.jpg.part", "dark/d.webp", "dark/deep/e.jpeg"] {
            std::fs::write(root.join(f), b"x").unwrap();
        }
        let names = |v: Vec<PathBuf>| v.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
        let mut all = names(list_images(&root, 100));
        all.sort();
        assert_eq!(all, ["a.jpg", "b.PNG", "d.webp", "e.jpeg"]);
        assert_eq!(list_images(&root, 2).len(), 2, "the cap stops the whole walk");
        assert!(list_images(&root.join("missing"), 10).is_empty());
        let _ = std::fs::remove_dir_all(&root);
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
