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

    #[test]
    fn set_dotted_creates_objects() {
        let mut v = json!({ "keep": 1 });
        set_dotted(&mut v, "appearance.transparency.enabled", json!(true));
        assert_eq!(v["appearance"]["transparency"]["enabled"], json!(true));
        assert_eq!(v["keep"], json!(1));
    }
}
