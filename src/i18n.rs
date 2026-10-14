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
    match is_es().then(|| es(s)).flatten() {
        Some(t) => t.to_owned(),
        None => s.to_owned(),
    }
}

/// Spanish for a Rust-side string, if known.
fn es(s: &str) -> Option<&'static str> {
    Some(match s {
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
        "Confetti" => "Confeti",
        "Celebrate with confetti on screen" => "Celebra con confeti en la pantalla",
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
        // launcher modes
        "Calculator" => "Calculadora",
        "Evaluate maths as you type (=2+2)" => "Calcula mientras escribes (=2+2)",
        "Emoji" => "Emoji",
        "Search and copy an emoji (:fire)" => "Busca y copia un emoji (:fuego)",
        "Clipboard" => "Portapapeles",
        "Paste from the clipboard history (;)" => "Pega desde el historial del portapapeles (;)",
        "Enter copies the result" => "Enter copia el resultado",
        "Type an expression, e.g. 2+2, sqrt(16)*pi, 90deg" => "Escribe una expresión, p. ej. 2+2, raiz(16)*pi, 90deg",
        "Undefined result" => "Resultado indefinido",
        "Unknown name" => "Nombre desconocido",
        "Keep typing…" => "Sigue escribiendo…",
        "No emoji found" => "No se encontró ningún emoji",
        "Try another word, in English or Spanish" => "Prueba otra palabra, en español o en inglés",
        "Clipboard history is off" => "El historial del portapapeles está desactivado",
        "Turn it on in Settings › Panels" => "Actívalo en Ajustes › Paneles",
        "Nothing copied yet" => "Aún no has copiado nada",
        "Text you copy appears here · Shift+Del removes an entry" => "Lo que copies aparece aquí · Mayús+Supr borra una entrada",
        "No matches" => "Sin coincidencias",
        // dots gallery
        "dynamic" => "dinámico",
        "Applied" => "Aplicado",
        "Previous dots restored" => "Dots anteriores restaurados",
        "no shell.json or scheme.json in that repository" => "ese repositorio no tiene shell.json ni scheme.json",
        "shell.json is not valid JSON" => "shell.json no es un JSON válido",
        "shell.json is not a JSON object" => "shell.json no es un objeto JSON",
        "that shell.json is not a Caelestia configuration" => "ese shell.json no es una configuración de Caelestia",
        "scheme.json is not a Caelestia scheme" => "scheme.json no es un esquema de Caelestia",
        "file too large" => "archivo demasiado grande",
        _ => return None,
    })
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

    /// `@tr("...")` strings in the .slint sources (escapes kept as written).
    fn ui_strings(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                ui_strings(&p, out);
            } else if p.extension().is_some_and(|x| x == "slint") {
                let text = std::fs::read_to_string(&p).unwrap();
                for (_, rest) in text.match_indices("@tr(\"").map(|(i, _)| ((), &text[i + 5..])) {
                    let mut s = String::new();
                    let mut chars = rest.chars();
                    while let Some(c) = chars.next() {
                        match c {
                            '\\' => {
                                s.push(c);
                                s.extend(chars.next());
                            }
                            '"' => break,
                            c => s.push(c),
                        }
                    }
                    out.push((s, p.file_name().unwrap().to_string_lossy().into_owned()));
                }
            }
        }
    }

    /// Every `@tr()` in the UI must have a Spanish translation.
    #[test]
    fn every_ui_string_is_translated() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let po = std::fs::read_to_string(root.join("translations/es/LC_MESSAGES/caelestia-win.po")).unwrap();
        let mut ids = std::collections::HashMap::new();
        let mut current: Option<String> = None;
        for line in po.lines() {
            if let Some(id) = line.strip_prefix("msgid \"").and_then(|l| l.strip_suffix('"')) {
                current = Some(id.to_owned());
            } else if let (Some(tr), Some(id)) = (line.strip_prefix("msgstr \"").and_then(|l| l.strip_suffix('"')), current.take()) {
                ids.insert(id, tr.to_owned());
            }
        }
        let mut used = Vec::new();
        ui_strings(&root.join("ui"), &mut used);
        let mut missing: Vec<String> = used.iter().filter(|(s, _)| ids.get(s).is_none_or(|t| t.is_empty())).map(|(s, f)| format!("{f}: {s}")).collect();
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "{} untranslated strings:\n{}", missing.len(), missing.join("\n"));
    }

    /// Strings shown from Rust (launcher actions and modes, dots import).
    #[test]
    fn rust_strings_are_translated() {
        let mut keys: Vec<&str> = crate::services::apps::ACTIONS.iter().flat_map(|a| [a.name, a.description]).collect();
        keys.extend([
            "Enter copies the result",
            "Type an expression, e.g. 2+2, sqrt(16)*pi, 90deg",
            "Undefined result",
            "Unknown name",
            "Keep typing…",
            "No emoji found",
            "Try another word, in English or Spanish",
            "Clipboard history is off",
            "Turn it on in Settings › Panels",
            "Nothing copied yet",
            "Text you copy appears here · Shift+Del removes an entry",
            "No matches",
            "Applied",
            "Previous dots restored",
            "no shell.json or scheme.json in that repository",
            "that shell.json is not a Caelestia configuration",
        ]);
        let missing: Vec<&&str> = keys.iter().filter(|k| es(k).is_none()).collect();
        assert!(missing.is_empty(), "no Spanish for {missing:?}");
    }

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
