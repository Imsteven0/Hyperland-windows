//! Dots gallery: imports someone's configuration from a GitHub link
//! (Settings › About › Dots). Reuses the wallpaper downloader's repository
//! index: one tree listing finds `shell.json`, `scheme.json` and the
//! wallpaper folder, then only those two small files are downloaded for a
//! preview. Applying keeps this machine's personal settings (paths,
//! language, weather location, autostart...) and drops anything that runs
//! commands; plugins are never imported.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use super::wallpaper_repo::{self, Blob, Repo};

/// Largest shell.json / scheme.json accepted.
const MAX_JSON: u64 = 512 * 1024;

/// Top-level keys of a Caelestia shell.json (to recognise one).
const SHELL_KEYS: [&str; 12] = ["appearance", "background", "bar", "border", "dashboard", "general", "launcher", "paths", "services", "session", "utilities", "windows"];

/// Settings that belong to this machine/user, never to shared dots.
const PERSONAL: [&str; 10] = [
    "paths",
    "general.language",
    "general.apps",
    "services.weatherLocation",
    "services.weatherLocationName",
    "windows.autostart",
    "bar.excludedScreens",
    "plugins",
    "launcher.hiddenApps",
    "windows.komorebi.binds",
];

/// Where the dots live inside the repository.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    pub shell: Option<String>,
    pub scheme: Option<String>,
    /// Folder with the most images, if its name looks like wallpapers.
    pub wallpapers: Option<(String, usize)>,
}

/// Finds the dots in a repository listing (optionally below `subdir`).
pub fn locate(blobs: &[Blob], subdir: &str) -> Layout {
    let prefix = if subdir.is_empty() { String::new() } else { format!("{}/", subdir.trim_matches('/')) };
    let inside: Vec<&str> = blobs.iter().map(|b| b.path.as_str()).filter(|p| p.starts_with(&prefix)).collect();
    // Prefer a path mentioning caelestia, then the shallowest one.
    let best = |name: &str, near: Option<&str>| -> Option<String> {
        inside
            .iter()
            .filter(|p| file_name(p).eq_ignore_ascii_case(name))
            .min_by_key(|p| (near.is_some_and(|d| parent(p) != d), !p.to_lowercase().contains("caelestia"), p.matches('/').count(), p.len()))
            .map(|p| (*p).to_owned())
    };
    let shell = best("shell.json", None);
    let scheme = best("scheme.json", shell.as_deref().map(parent));
    let mut folders: std::collections::BTreeMap<&str, usize> = Default::default();
    for p in inside.iter().filter(|p| wallpaper_repo::is_image(p)) {
        *folders.entry(parent(p)).or_default() += 1;
    }
    let wallpapers = folders
        .into_iter()
        .filter(|(dir, _)| dir.split('/').any(|seg| { let s = seg.to_lowercase(); s.contains("wall") || s.contains("fondo") || s.contains("background") }))
        .max_by_key(|(_, n)| *n)
        .map(|(dir, n)| (dir.to_owned(), n));
    Layout { shell, scheme, wallpapers }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// Checks a downloaded shell.json looks like Caelestia's.
pub fn validate_shell(text: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(text).context("shell.json is not valid JSON")?;
    let Some(obj) = v.as_object() else { bail!("shell.json is not a JSON object") };
    if !obj.keys().any(|k| SHELL_KEYS.contains(&k.as_str())) {
        bail!("that shell.json is not a Caelestia configuration");
    }
    Ok(v)
}

/// The imported config with this machine's personal settings kept as they
/// are (or left out when the user never set them).
pub fn merge_imported(current: &Value, imported: &Value) -> Value {
    let mut out = imported.clone();
    for key in PERSONAL {
        remove_dotted(&mut out, key);
        if let Some(mine) = get_dotted(current, key) {
            set_dotted(&mut out, key, mine.clone());
        }
    }
    out
}

fn get_dotted<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |cur, k| cur.get(k))
}

fn remove_dotted(v: &mut Value, path: &str) {
    let (parent_path, last) = path.rsplit_once('.').unwrap_or(("", path));
    let parent = if parent_path.is_empty() { Some(v) } else { parent_path.split('.').try_fold(v, |cur, k| cur.get_mut(k)) };
    if let Some(Value::Object(o)) = parent {
        o.remove(last);
    }
}

fn set_dotted(v: &mut Value, path: &str, value: Value) {
    let mut cur = v;
    let parts: Vec<&str> = path.split('.').collect();
    for (i, k) in parts.iter().enumerate() {
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        let obj = cur.as_object_mut().expect("object");
        if i + 1 == parts.len() {
            obj.insert((*k).to_owned(), value);
            return;
        }
        cur = obj.entry((*k).to_owned()).or_insert_with(|| Value::Object(Map::new()));
    }
}

/// What a repository offers, ready to apply.
#[derive(Debug, Clone)]
pub struct Preview {
    pub repo: Repo,
    pub branch: String,
    pub shell: Option<Value>,
    pub scheme: Option<crate::theme::Scheme>,
    pub layout: Layout,
}

impl Preview {
    /// Link for the wallpaper downloader (`.../tree/<branch>/<folder>`).
    pub fn wallpapers_url(&self) -> Option<String> {
        self.layout.wallpapers.as_ref().map(|(dir, _)| format!("https://github.com/{}/{}/tree/{}/{dir}", self.repo.owner, self.repo.name, self.branch))
    }
}

/// Indexes the repository and downloads its dots (blocking; run off the UI thread).
pub fn fetch(url: &str) -> Result<Preview> {
    let repo = Repo::parse(url).context("Not a GitHub repository link")?;
    let agent = wallpaper_repo::agent();
    let (branch, blobs) = wallpaper_repo::tree(&agent, &repo)?;
    let layout = locate(&blobs, &repo.subdir);
    if layout.shell.is_none() && layout.scheme.is_none() {
        bail!("no shell.json or scheme.json in that repository");
    }
    let download = |path: &str| wallpaper_repo::get_text(&agent, &wallpaper_repo::raw_url(&repo, &branch, path), MAX_JSON);
    let shell = layout.shell.as_deref().map(|p| download(p).and_then(|t| validate_shell(&t))).transpose()?;
    let scheme = match layout.scheme.as_deref() {
        Some(p) => {
            let text = download(p)?;
            Some(serde_json::from_str::<crate::theme::Scheme>(&text).context("scheme.json is not a Caelestia scheme")?)
        }
        None => None,
    };
    Ok(Preview { repo, branch, shell, scheme, layout })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn blobs(paths: &[&str]) -> Vec<Blob> {
        paths.iter().map(|p| Blob { path: (*p).to_owned(), size: 1 }).collect()
    }

    #[test]
    fn locates_dots_in_common_layouts() {
        let b = blobs(&[
            "README.md",
            "other/shell.json",
            ".config/caelestia/shell.json",
            ".config/caelestia/scheme.json",
            "themes/scheme.json",
            "Wallpapers/a.jpg",
            "Wallpapers/b.png",
            "Wallpapers/dark/c.jpg",
            "screenshots/1.png",
            "screenshots/2.png",
            "screenshots/3.png",
        ]);
        let l = locate(&b, "");
        assert_eq!(l.shell.as_deref(), Some(".config/caelestia/shell.json"));
        assert_eq!(l.scheme.as_deref(), Some(".config/caelestia/scheme.json"));
        assert_eq!(l.wallpapers, Some(("Wallpapers".into(), 2)), "screenshots are not wallpapers");
    }

    #[test]
    fn locates_inside_a_subfolder_and_handles_missing_parts() {
        let b = blobs(&["a/shell.json", "b/shell.json", "b/scheme.json"]);
        let l = locate(&b, "b");
        assert_eq!((l.shell.as_deref(), l.scheme.as_deref()), (Some("b/shell.json"), Some("b/scheme.json")));
        let l = locate(&blobs(&["x/scheme.json"]), "");
        assert_eq!((l.shell, l.scheme.as_deref(), l.wallpapers), (None, Some("x/scheme.json"), None));
        assert_eq!(locate(&[], ""), Layout::default());
    }

    #[test]
    fn validates_shell_json() {
        assert!(validate_shell(r#"{ "bar": { "persistent": true } }"#).is_ok());
        assert!(validate_shell("[1, 2]").is_err());
        assert!(validate_shell(r#"{ "name": "some-npm-package" }"#).is_err());
        assert!(validate_shell("{ nope").is_err());
    }

    #[test]
    fn keeps_personal_settings_and_drops_commands() {
        let mine = json!({
            "paths": { "wallpaperDir": "D:/Fondos" },
            "general": { "language": "es" },
            "services": { "weatherLocation": "9.9,-84.1", "useFahrenheit": false },
            "windows": { "autostart": false }
        });
        let theirs = json!({
            "appearance": { "rounding": { "scale": 1.5 } },
            "paths": { "wallpaperDir": "~/their/walls" },
            "general": { "language": "en", "apps": { "terminal": ["evil.exe"] } },
            "services": { "weatherLocation": "40.7,-74.0", "useFahrenheit": true },
            "windows": { "autostart": true, "blur": false },
            "plugins": { "disabled": ["x"] }
        });
        let out = merge_imported(&mine, &theirs);
        assert_eq!(out["appearance"]["rounding"]["scale"], json!(1.5));
        assert_eq!(out["paths"], json!({ "wallpaperDir": "D:/Fondos" }));
        assert_eq!(out["general"], json!({ "language": "es" }), "their commands never come in");
        assert_eq!(out["services"], json!({ "weatherLocation": "9.9,-84.1", "useFahrenheit": true }));
        assert_eq!(out["windows"], json!({ "autostart": false, "blur": false }));
        assert!(out.get("plugins").is_none());
    }

    #[test]
    fn builds_the_wallpaper_link() {
        let p = Preview {
            repo: Repo::parse("https://github.com/o/r").unwrap(),
            branch: "main".into(),
            shell: None,
            scheme: None,
            layout: Layout { wallpapers: Some(("assets/Wallpapers".into(), 3)), ..Default::default() },
        };
        assert_eq!(p.wallpapers_url().as_deref(), Some("https://github.com/o/r/tree/main/assets/Wallpapers"));
        assert_eq!(Repo::parse(&p.wallpapers_url().unwrap()).unwrap().subdir, "assets/Wallpapers");
    }
}
