//! Runtime font registration. Caelestia's look depends on Rubik and
//! Material Symbols Rounded; they're loaded from `assets/fonts` (next to the
//! executable or the repo) and from `<config>/fonts`, so dots can ship fonts.

use std::{path::PathBuf, sync::Arc};

use slint::fontique_011::fontique;

pub fn register(config_dir: &std::path::Path) -> Vec<String> {
    let mut dirs: Vec<PathBuf> = vec![config_dir.join("fonts")];
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)) {
        dirs.push(exe_dir.join("assets").join("fonts"));
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join("fonts"));

    let mut collection = slint::fontique_011::shared_collection();
    let mut families = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
            if !matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc")) {
                continue;
            }
            match std::fs::read(&path) {
                Ok(bytes) => {
                    for (family, _) in collection.register_fonts(fontique::Blob::new(Arc::new(bytes)), None) {
                        if let Some(name) = collection.family_name(family) {
                            families.push(name.to_owned());
                        }
                    }
                }
                Err(e) => log::warn!("font {}: {e}", path.display()),
            }
        }
    }
    families.sort();
    families.dedup();
    families
}
