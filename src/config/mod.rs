//! Dots: `~/.config/caelestia/shell.json` (same location and schema as
//! Caelestia). The user's file is kept verbatim as a `Value` so writes from the
//! settings panel never drop keys this port does not understand; reads go
//! through `merged`, which is the user's file layered over the defaults.

mod defaults;
pub mod watch;

use std::{
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde_json::{Map, Value};

pub use defaults::default_config;

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub shell_json: PathBuf,
    pub state_dir: PathBuf,
    pub scheme_json: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let config_dir = std::env::var_os("CAELESTIA_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config").join("caelestia"));
        let state_dir = std::env::var_os("CAELESTIA_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local").join("state").join("caelestia"));
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| home.join(".cache"))
            .join("caelestia");
        Self {
            shell_json: config_dir.join("shell.json"),
            scheme_json: state_dir.join("scheme.json"),
            config_dir,
            state_dir,
            cache_dir,
        }
    }
}

pub struct Config {
    pub paths: Paths,
    user: Value,
    merged: Value,
    /// Hash of the last content we wrote, so the watcher can ignore our own saves.
    last_written: Option<u64>,
}

impl Config {
    pub fn load(paths: Paths) -> Result<Self> {
        fs::create_dir_all(&paths.config_dir)?;
        fs::create_dir_all(&paths.state_dir)?;
        fs::create_dir_all(&paths.cache_dir)?;

        let mut cfg = Self { paths, user: Value::Object(Map::new()), merged: default_config(), last_written: None };
        if cfg.paths.shell_json.exists() {
            cfg.reload()?;
        } else {
            // First run: write a fully populated file so users can discover every option.
            cfg.user = default_config();
            cfg.merged = default_config();
            cfg.save()?;
        }
        Ok(cfg)
    }

    /// Re-reads shell.json. Returns `false` when the content is our own last write.
    pub fn reload(&mut self) -> Result<bool> {
        let text = fs::read_to_string(&self.paths.shell_json)
            .with_context(|| format!("reading {}", self.paths.shell_json.display()))?;
        if self.last_written == Some(hash_str(&text)) {
            return Ok(false);
        }
        let user: Value = if text.trim().is_empty() {
            Value::Object(Map::new())
        } else {
            serde_json::from_str(&text).with_context(|| format!("parsing {}", self.paths.shell_json.display()))?
        };
        self.set_user(user);
        Ok(true)
    }

    pub fn replace_with(&mut self, user: Value) -> Result<()> {
        self.set_user(user);
        self.save()
    }

    fn set_user(&mut self, user: Value) {
        let mut merged = default_config();
        deep_merge(&mut merged, &user);
        self.user = user;
        self.merged = merged;
    }

    pub fn save(&mut self) -> Result<()> {
        let text = serde_json::to_string_pretty(&self.user)? + "\n";
        write_atomic(&self.paths.shell_json, &text)?;
        self.last_written = Some(hash_str(&text));
        Ok(())
    }

    pub fn user_value(&self) -> &Value {
        &self.user
    }

    /// Sets a dotted path (e.g. `appearance.transparency.enabled`) in the user
    /// file. `bar.entries.<id>` toggles the matching entry like Caelestia does.
    pub fn set(&mut self, path: &str, value: Value) -> Result<()> {
        if let Some(id) = path.strip_prefix("bar.entries.") {
            let entries = self.merged.pointer("/bar/entries").cloned().unwrap_or(Value::Array(vec![]));
            let mut entries = entries.as_array().cloned().unwrap_or_default();
            let mut found = false;
            for e in entries.iter_mut() {
                if e.get("id").and_then(Value::as_str) == Some(id) {
                    e["enabled"] = value.clone();
                    found = true;
                }
            }
            if !found {
                entries.push(serde_json::json!({ "id": id, "enabled": value }));
            }
            set_dotted(&mut self.user, "bar.entries", Value::Array(entries));
        } else {
            set_dotted(&mut self.user, path, value);
        }
        let user = std::mem::take(&mut self.user);
        self.set_user(user);
        Ok(())
    }

    // ---------- typed accessors over the merged view ----------

    pub fn get(&self, path: &str) -> Option<&Value> {
        let mut cur = &self.merged;
        for part in path.split('.') {
            cur = cur.get(part)?;
        }
        Some(cur)
    }
    pub fn bool(&self, path: &str) -> bool {
        self.get(path).and_then(Value::as_bool).unwrap_or(false)
    }
    pub fn f64(&self, path: &str) -> f64 {
        self.get(path).and_then(Value::as_f64).unwrap_or(0.0)
    }
    pub fn f32(&self, path: &str) -> f32 {
        self.f64(path) as f32
    }
    pub fn i64(&self, path: &str) -> i64 {
        self.get(path).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f.round() as i64))).unwrap_or(0)
    }
    pub fn str(&self, path: &str) -> String {
        self.get(path).and_then(Value::as_str).unwrap_or_default().to_owned()
    }
    pub fn path(&self, path: &str) -> PathBuf {
        expand_home(&self.str(path))
    }
    pub fn str_list(&self, path: &str) -> Vec<String> {
        self.get(path)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default()
    }
    /// Whether a bar entry (`logo`, `workspaces`, `clock`...) is enabled.
    pub fn bar_entry(&self, id: &str) -> bool {
        self.get("bar.entries")
            .and_then(Value::as_array)
            .map(|entries| {
                entries.iter().any(|e| {
                    e.get("id").and_then(Value::as_str) == Some(id)
                        && e.get("enabled").and_then(Value::as_bool).unwrap_or(true)
                })
            })
            .unwrap_or(true)
    }
}

pub fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}

pub fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn hash_str(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

pub fn deep_merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(slot) => deep_merge(slot, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (slot, v) => *slot = v.clone(),
    }
}

fn set_dotted(root: &mut Value, path: &str, value: Value) {
    let mut cur = root;
    let parts: Vec<&str> = path.split('.').collect();
    for (i, part) in parts.iter().enumerate() {
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        let obj = cur.as_object_mut().expect("object");
        if i == parts.len() - 1 {
            obj.insert((*part).to_owned(), value);
            return;
        }
        cur = obj.entry((*part).to_owned()).or_insert_with(|| Value::Object(Map::new()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_keeps_unknown_keys_and_overrides() {
        let mut base = json!({ "a": { "b": 1, "c": 2 } });
        deep_merge(&mut base, &json!({ "a": { "b": 5, "x": true }, "y": 1 }));
        assert_eq!(base, json!({ "a": { "b": 5, "c": 2, "x": true }, "y": 1 }));
    }

    fn temp_paths(name: &str) -> Paths {
        let root = std::env::temp_dir().join(format!("caelestia-config-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        Paths {
            shell_json: root.join("config").join("shell.json"),
            scheme_json: root.join("state").join("scheme.json"),
            config_dir: root.join("config"),
            state_dir: root.join("state"),
            cache_dir: root.join("cache"),
        }
    }

    #[test]
    fn first_run_writes_the_full_defaults() {
        let paths = temp_paths("first");
        let cfg = Config::load(paths.clone()).unwrap();
        let written: Value = serde_json::from_str(&fs::read_to_string(&paths.shell_json).unwrap()).unwrap();
        assert_eq!(written, default_config());
        assert_eq!(cfg.str("launcher.actionPrefix"), ">");
        let _ = fs::remove_dir_all(paths.config_dir.parent().unwrap());
    }

    #[test]
    fn user_file_overrides_defaults_and_keeps_unknown_keys() {
        let paths = temp_paths("merge");
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(&paths.shell_json, r#"{ "border": { "thickness": 4 }, "someLinuxOnlyKey": { "x": 1 } }"#).unwrap();
        let mut cfg = Config::load(paths.clone()).unwrap();
        assert_eq!(cfg.i64("border.thickness"), 4);
        assert_eq!(cfg.i64("border.rounding"), 25, "untouched keys come from the defaults");
        // Saving from the settings panel writes only the user's keys (plus the new one).
        cfg.set("appearance.transparency.enabled", json!(true)).unwrap();
        cfg.save().unwrap();
        let written: Value = serde_json::from_str(&fs::read_to_string(&paths.shell_json).unwrap()).unwrap();
        assert_eq!(written["someLinuxOnlyKey"], json!({ "x": 1 }));
        assert_eq!(written["appearance"], json!({ "transparency": { "enabled": true } }));
        assert!(written.get("bar").is_none(), "defaults are never copied into the user file");
        let _ = fs::remove_dir_all(paths.config_dir.parent().unwrap());
    }

    #[test]
    fn reload_ignores_our_own_writes_and_keeps_config_on_errors() {
        let paths = temp_paths("reload");
        let mut cfg = Config::load(paths.clone()).unwrap();
        cfg.set("border.thickness", json!(7)).unwrap();
        cfg.save().unwrap();
        assert!(!cfg.reload().unwrap(), "the watcher sees our own save");
        fs::write(&paths.shell_json, r#"{ "border": { "thickness": 12 } }"#).unwrap();
        assert!(cfg.reload().unwrap());
        assert_eq!(cfg.i64("border.thickness"), 12);
        fs::write(&paths.shell_json, "{ broken").unwrap();
        assert!(cfg.reload().is_err());
        assert_eq!(cfg.i64("border.thickness"), 12, "a broken file keeps the previous config");
        fs::write(&paths.shell_json, "   ").unwrap();
        assert!(cfg.reload().unwrap());
        assert_eq!(cfg.i64("border.thickness"), 10, "an empty file means all defaults");
        let _ = fs::remove_dir_all(paths.config_dir.parent().unwrap());
    }

    #[test]
    fn bar_entries_toggle_like_caelestia() {
        let paths = temp_paths("entries");
        let mut cfg = Config::load(paths.clone()).unwrap();
        assert!(cfg.bar_entry("clock"));
        cfg.set("bar.entries.clock", json!(false)).unwrap();
        assert!(!cfg.bar_entry("clock"));
        assert!(cfg.bar_entry("workspaces"), "other entries keep their state");
        cfg.set("bar.entries.newWidget", json!(false)).unwrap();
        assert!(!cfg.bar_entry("newWidget"));
        assert!(!cfg.bar_entry("neverMentioned"), "with an entries list, only listed entries show (like Caelestia)");
        let _ = fs::remove_dir_all(paths.config_dir.parent().unwrap());
    }

    #[test]
    fn typed_accessors() {
        let paths = temp_paths("typed");
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(&paths.shell_json, r#"{ "a": { "f": 2.6, "s": "x", "list": ["a", 1, "b"], "b": "true" } }"#).unwrap();
        let cfg = Config::load(paths.clone()).unwrap();
        assert_eq!(cfg.i64("a.f"), 3, "floats round when an integer is wanted");
        assert_eq!(cfg.f32("a.f"), 2.6);
        assert_eq!(cfg.str_list("a.list"), ["a", "b"]);
        assert!(!cfg.bool("a.b"), "strings are not booleans");
        assert_eq!(cfg.str("a.missing"), "");
        assert_eq!(cfg.i64("a.s"), 0);
        let _ = fs::remove_dir_all(paths.config_dir.parent().unwrap());
    }

    /// Every key the UI writes must be one the shell reads: a typo (or a
    /// renamed option) would make a Settings switch silently do nothing.
    #[test]
    fn every_ui_setting_key_exists() {
        let defaults = default_config();
        let entries: Vec<String> = defaults["bar"]["entries"].as_array().unwrap().iter().filter_map(|e| e["id"].as_str().map(str::to_owned)).collect();
        let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let mut files = vec![ui.clone()];
        let mut keys = Vec::new();
        while let Some(p) = files.pop() {
            if p.is_dir() {
                files.extend(fs::read_dir(&p).unwrap().flatten().map(|e| e.path()));
            } else if p.extension().is_some_and(|e| e == "slint") {
                let text = fs::read_to_string(&p).unwrap();
                for marker in ["key: \"", "Shell.set-config(\""] {
                    for (i, _) in text.match_indices(marker) {
                        let rest = &text[i + marker.len()..];
                        keys.push(rest[..rest.find('"').unwrap()].to_owned());
                    }
                }
            }
        }
        assert!(keys.len() > 20, "found only {keys:?}");
        let unknown: Vec<&String> = keys
            .iter()
            // `@...` are actions; services.scheme.* is stored in scheme.json by set_config.
            .filter(|k| !k.starts_with('@') && !k.starts_with("services.scheme."))
            .filter(|k| match k.strip_prefix("bar.entries.") {
                Some(id) => !entries.iter().any(|e| e == id),
                None => k.split('.').try_fold(&defaults, |v, part| v.get(part)).is_none(),
            })
            .collect();
        assert!(unknown.is_empty(), "UI writes keys missing from the defaults: {unknown:?}");
    }

    /// schema/shell.schema.json (editor autocompletion for the dots) must
    /// describe every default key, with a matching type.
    #[test]
    fn schema_covers_every_default() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("schema").join("shell.schema.json");
        let schema: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        fn walk(v: &Value, s: &Value, at: &str, missing: &mut Vec<String>) {
            let ty = s["type"].as_str().unwrap_or("");
            let ok = match v {
                Value::Bool(_) => ty == "boolean",
                Value::Number(n) => ty == "number" || (ty == "integer" && n.is_i64()),
                Value::String(_) => ty == "string" || s.get("$ref").is_some(),
                Value::Array(_) => ty == "array",
                Value::Object(_) => ty == "object",
                Value::Null => true,
            };
            if !ok {
                missing.push(format!("{at}: wrong type {ty:?}"));
            }
            if let Value::Object(o) = v {
                for (k, child) in o {
                    let path = if at.is_empty() { k.clone() } else { format!("{at}.{k}") };
                    match s["properties"].get(k) {
                        Some(cs) if cs.get("description").is_some() || k == "$schema" || cs.get("$ref").is_some() => walk(child, cs, &path, missing),
                        Some(cs) if child.is_object() => walk(child, cs, &path, missing),
                        Some(_) => missing.push(format!("{path}: no description")),
                        None if s.get("additionalProperties").is_some_and(Value::is_object) => {}
                        None => missing.push(format!("{path}: not in the schema")),
                    }
                }
            }
        }
        let mut missing = Vec::new();
        walk(&default_config(), &schema, "", &mut missing);
        assert!(missing.is_empty(), "{missing:#?}");
        assert_eq!(schema["$id"], defaults::SCHEMA_URL);
    }

    #[test]
    fn expands_home() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home("~/Pictures"), home.join("Pictures"));
        assert_eq!(expand_home("~\\Videos"), home.join("Videos"));
        assert_eq!(expand_home("D:/Walls"), PathBuf::from("D:/Walls"));
    }

    #[test]
    fn set_dotted_creates_objects() {
        let mut v = json!({ "keep": 1 });
        set_dotted(&mut v, "appearance.transparency.enabled", json!(true));
        assert_eq!(v["appearance"]["transparency"]["enabled"], json!(true));
        assert_eq!(v["keep"], json!(1));
    }
}
