//! Hot reload: watches the dots directories and reports which file changed.
//! Directories (not files) are watched so atomic renames by editors work.

use std::{path::PathBuf, time::Duration};

use anyhow::Result;
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer, notify::RecommendedWatcher, notify::RecursiveMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Changed {
    Shell,
    Scheme,
}

pub struct Watcher {
    _debouncer: Debouncer<RecommendedWatcher>,
}

impl Watcher {
    pub fn spawn(shell_json: PathBuf, scheme_json: PathBuf, on_change: impl Fn(Changed) + Send + 'static) -> Result<Self> {
        let (shell_c, scheme_c) = (shell_json.clone(), scheme_json.clone());
        let mut debouncer = new_debouncer(Duration::from_millis(250), move |res: DebounceEventResult| {
            let Ok(events) = res else { return };
            let mut shell = false;
            let mut scheme = false;
            for e in events {
                shell |= e.path == shell_c;
                scheme |= e.path == scheme_c;
            }
            if shell {
                on_change(Changed::Shell);
            }
            if scheme {
                on_change(Changed::Scheme);
            }
        })?;
        for dir in [shell_json.parent(), scheme_json.parent()].into_iter().flatten() {
            debouncer.watcher().watch(dir, RecursiveMode::NonRecursive)?;
        }
        Ok(Self { _debouncer: debouncer })
    }
}
