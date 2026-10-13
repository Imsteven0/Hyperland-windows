//! Language selection and the few strings produced in Rust (launcher
//! actions, weather descriptions, uptime, dates). Slint strings use @tr()
//! with the bundled `translations/<lang>/LC_MESSAGES/caelestia-win.po`.

use std::sync::RwLock;

static LANG: RwLock<String> = RwLock::new(String::new());

/// Resolves `general.language` ("", "auto", "en", "es"...) to a language code.
pub fn resolve(setting: &str, system: &str) -> String {
    match setting.trim() {
        "" | "auto" => system.to_owned(),
        l => l.to_lowercase(),
    }
}

/// Applies the language to Slint (@tr) and to Rust-side strings.
pub fn set_language(lang: &str) {
    let supported = if lang == "es" { "es" } else { "en" };
    if let Err(e) = slint::select_bundled_translation(supported) {
        log::warn!("translation {supported}: {e:?}");
    }
    *LANG.write().unwrap() = supported.to_owned();
}

pub fn is_es() -> bool {
    LANG.read().map(|l| l.as_str() == "es").unwrap_or(false)
}

pub fn lang() -> String {
    LANG.read().map(|l| if l.is_empty() { "en".into() } else { l.clone() }).unwrap_or_else(|_| "en".into())
}

/// Translates a Rust-side English string (identity in English).
pub fn tr(s: &str) -> String {
    if !is_es() {
        return s.to_owned();
    }
    let t = match s {
        // launcher actions
        "Wallpaper" => "Fondo",
        "Change the current wallpaper" => "Cambia el fondo actual",
        "Random" => "Aleatorio",
        "Switch to a random wallpaper" => "Pon un fondo aleatorio",
        "Light" => "Claro",
        "Change the scheme to light mode" => "Cambia el esquema a modo claro",
        "Dark" => "Oscuro",
        "Change the scheme to dark mode" => "Cambia el esquema a modo oscuro",
        "Variant" => "Variante",
        "Pick the dynamic scheme variant (live preview)" => "Elige la variante del esquema dinámico (vista previa en vivo)",
        "Transparency" => "Transparencia",
        "Pick the panel opacity theme (live preview)" => "Elige el tema de opacidad de los paneles (vista previa en vivo)",
        "Scheme" => "Esquema",
        "Pick a colour scheme (live preview)" => "Elige un esquema de color (vista previa en vivo)",
        "Settings" => "Ajustes",
        "Open the shell settings" => "Abre los ajustes del shell",
        "Dashboard" => "Panel",
        "Open the dashboard" => "Abre el panel",
        "Reload" => "Recargar",
        "Reload shell.json and scheme.json" => "Recarga shell.json y scheme.json",
        "Lock" => "Bloquear",
        "Lock the session" => "Bloquea la sesión",
        "Sleep" => "Suspender",
        "Suspend the computer" => "Suspende el equipo",
        "Logout" => "Cerrar sesión",
        "Sign out of the session" => "Cierra la sesión",
        "Reboot" => "Reiniciar",
        "Restart the computer" => "Reinicia el equipo",
        "Shutdown" => "Apagar",
        "Power off the computer" => "Apaga el equipo",
        "Quit shell" => "Salir del shell",
        "Exit Caelestia and free the reserved screen space" => "Cierra Caelestia y libera el espacio reservado",
        // weather
        "Clear sky" => "Despejado",
        "Mainly clear" => "Mayormente despejado",
        "Partly cloudy" => "Parcialmente nublado",
        "Overcast" => "Nublado",
        "Fog" => "Niebla",
        "Drizzle" => "Llovizna",
        "Freezing drizzle" => "Llovizna helada",
        "Rain" => "Lluvia",
        "Freezing rain" => "Lluvia helada",
        "Snow" => "Nieve",
        "Rain showers" => "Chubascos",
        "Snow showers" => "Chubascos de nieve",
        "Thunderstorm" => "Tormenta",
        "Thunderstorm with hail" => "Tormenta con granizo",
        "Unknown" => "Desconocido",
        "Today" => "Hoy",
        "Current location" => "Ubicación actual",
        // misc
        "Dynamic (wallpaper)" => "Dinámico (fondo)",
        "Hostname" => "Equipo",
        "Device" => "Fabricante",
        "OS" => "Sistema",
        "Build" => "Compilación",
        "Firmware" => "Firmware",
        "Shell" => "Shell",
        "UI toolkit" => "Toolkit de interfaz",
        "Renderer" => "Renderizador",
        "Config format" => "Formato de configuración",
        "Desktop" => "Escritorio",
        "Solid" => "Sólido",
        "Subtle" => "Sutil",
        "Translucent" => "Translúcido",
        "Glass" => "Cristal",
        "Clear" => "Transparente",
        "Opacity" => "Opacidad",
        "Colour scheme" => "Esquema de color",
        "Dynamic variant" => "Variante dinámica",
        "Current" => "Actual",
        "Error" => "Error",
        "Not a GitHub repository link" => "No es un enlace de un repositorio de GitHub",
        "Reading the repository…" => "Leyendo el repositorio…",
        "repository not found" => "repositorio no encontrado",
        "GitHub rate limit reached, try again later" => "límite de GitHub alcanzado, inténtalo más tarde",
        "no images in that repository" => "ese repositorio no tiene imágenes",
        other => return other.to_owned(),
    };
    t.to_owned()
}

/// Locale for chrono date formatting.
pub fn chrono_locale() -> chrono::Locale {
    if is_es() { chrono::Locale::es_ES } else { chrono::Locale::en_US }
}

/// "up 2h 5m" / "activo 2 h 5 min".
pub fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    if is_es() {
        match (d, h) {
            (0, 0) => format!("activo {m} min"),
            (0, _) => format!("activo {h} h {m} min"),
            _ => format!("activo {d} d {h} h"),
        }
    } else {
        match (d, h) {
            (0, 0) => format!("up {m} min"),
            (0, _) => format!("up {h}h {m}m"),
            _ => format!("up {d}d {h}h"),
        }
    }
}

/// "now", "5 min ago", "2 h ago", "3 d ago" in the UI language.
pub fn ago(secs: u64) -> String {
    ago_in(secs, is_es())
}

fn ago_in(secs: u64, es: bool) -> String {
    let (m, h, d) = (secs / 60, secs / 3600, secs / 86400);
    match (es, secs) {
        (true, 0..60) => "ahora".into(),
        (false, 0..60) => "now".into(),
        (true, 60..3600) => format!("hace {m} min"),
        (false, 60..3600) => format!("{m} min ago"),
        (true, 3600..86400) => format!("hace {h} h"),
        (false, 3600..86400) => format!("{h} h ago"),
        (true, _) => format!("hace {d} d"),
        (false, _) => format!("{d} d ago"),
    }
}

/// Capitalises the first letter (Spanish day/month names are lowercase).
pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times() {
        assert_eq!(ago_in(5, false), "now");
        assert_eq!(ago_in(5, true), "ahora");
        assert_eq!(ago_in(125, false), "2 min ago");
        assert_eq!(ago_in(125, true), "hace 2 min");
        assert_eq!(ago_in(7200, true), "hace 2 h");
        assert_eq!(ago_in(3 * 86400 + 5, false), "3 d ago");
    }

    #[test]
    fn resolves_language_setting() {
        assert_eq!(resolve("auto", "es"), "es");
        assert_eq!(resolve("", "en"), "en");
        assert_eq!(resolve(" ES ", "en"), "es");
    }

    #[test]
    fn capitalizes_unicode() {
        assert_eq!(capitalize("miércoles"), "Miércoles");
        assert_eq!(capitalize("ñandú"), "Ñandú");
        assert_eq!(capitalize(""), "");
    }
}
