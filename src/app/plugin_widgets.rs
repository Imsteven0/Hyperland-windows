//! Plugins: loading, results and clicks.

use super::*;

impl Caelestia {
    /// (Re)reads `<config>/plugins` and restarts the enabled plugins.
    pub(super) fn load_plugins(&self) {
        let (dir, disabled) = {
            let cfg = self.cfg.borrow();
            (cfg.paths.config_dir.join("plugins"), cfg.str_list("plugins.disabled"))
        };
        let (found, errors) = plugins::discover(&dir);
        let mut listing: Vec<PluginListing> = found
            .iter()
            .map(|p| PluginListing { id: p.id.clone(), name: p.manifest.name.clone(), icon: p.manifest.icon.clone(), enabled: !disabled.contains(&p.id), error: String::new() })
            .collect();
        listing.extend(errors.into_iter().map(|(id, e)| {
            log::warn!("plugin {id}: {e}");
            PluginListing { name: id.clone(), id, icon: "error".into(), enabled: false, error: e }
        }));
        listing.sort_by(|a, b| a.id.cmp(&b.id));
        let enabled: Vec<Plugin> = found.into_iter().filter(|p| !disabled.contains(&p.id)).collect();
        log::info!("plugins: {} enabled of {}", enabled.len(), listing.len());
        let generation = self.plugin_runner.borrow_mut().as_mut().map(|r| r.load(enabled.clone())).unwrap_or(0);
        {
            let mut st = self.st.borrow_mut();
            st.plugin_results = vec![None; enabled.len()];
            st.plugins = enabled;
            st.plugin_listing = listing;
            st.plugin_gen = generation;
        }
        self.push_plugins();
    }

    pub(super) fn on_plugin_update(&self, u: plugins::Update) {
        {
            let mut st = self.st.borrow_mut();
            if u.generation != st.plugin_gen {
                return; // from before a reload
            }
            if let Err(e) = &u.result {
                log::warn!("plugin {}: {e}", st.plugins.get(u.index).map(|p| p.id.as_str()).unwrap_or("?"));
            }
            match st.plugin_results.get_mut(u.index) {
                Some(slot) if slot.as_ref() != Some(&u.result) => *slot = Some(u.result),
                _ => return, // unchanged: don't rebuild the models
            }
        }
        self.push_plugins();
    }

    /// Plugin models for the bar, the dashboard and Settings.
    pub(super) fn push_plugins(&self) {
        let st = self.st.borrow();
        let widget = |i: usize, p: &Plugin| {
            let (out, error) = match st.plugin_results.get(i).cloned().flatten() {
                Some(Ok(o)) => (o, String::new()),
                Some(Err(e)) => (plugins::Output::default(), e),
                None => (plugins::Output::default(), String::new()),
            };
            PluginWidget {
                index: i as i32,
                id: p.id.clone().into(),
                name: p.manifest.name.clone().into(),
                icon: out.icon.clone().unwrap_or_else(|| p.manifest.icon.clone()).into(),
                text: if error.is_empty() { out.text.clone() } else { "!".into() }.into(),
                tooltip: if error.is_empty() { out.tooltip.clone() } else { error.clone() }.into(),
                tone: if error.is_empty() { out.tone.id() } else { "critical" }.into(),
                percentage: out.percentage.unwrap_or(0.0),
                has_percentage: out.percentage.is_some(),
                error: error.into(),
                enabled: true,
                clickable: p.manifest.on_click.is_some(),
            }
        };
        let of = |loc: plugins::Location| -> Vec<PluginWidget> { st.plugins.iter().enumerate().filter(|(_, p)| p.manifest.location == loc).map(|(i, p)| widget(i, p)).collect() };
        let bar = ModelRc::new(VecModel::from(of(plugins::Location::Bar)));
        let dash = ModelRc::new(VecModel::from(of(plugins::Location::Dashboard)));
        let listing: Vec<PluginWidget> = st
            .plugin_listing
            .iter()
            .map(|l| PluginWidget {
                index: -1,
                id: l.id.clone().into(),
                name: l.name.clone().into(),
                icon: l.icon.clone().into(),
                error: l.error.clone().into(),
                enabled: l.enabled,
                ..Default::default()
            })
            .collect();
        drop(st);
        for s in self.screens.borrow().iter() {
            s.bar.global::<ShellGlobal>().set_bar_plugins(bar.clone());
        }
        self.dashboard.global::<ShellGlobal>().set_dash_plugins(dash);
        self.settings.global::<ShellGlobal>().set_plugin_list(ModelRc::new(VecModel::from(listing)));
    }

    pub(super) fn plugin_click(&self, index: usize) {
        let Some(p) = self.st.borrow().plugins.get(index).cloned() else { return };
        let Some(spec) = &p.manifest.on_click else { return };
        let argv = plugins::resolve(spec, &p.dir);
        // A lone URI (ms-settings:, https://...) opens with its handler.
        if let [uri] = argv.as_slice() {
            if uri.contains(':') && !Path::new(uri).is_file() {
                system::open(uri);
                return;
            }
        }
        let (dir, timeout) = (p.dir.clone(), p.timeout());
        std::thread::spawn(move || {
            if let Err(e) = plugins::run(&argv, &dir, timeout.max(Duration::from_secs(60))) {
                log::warn!("plugin click: {e}");
            }
            // Refresh the widget: the click probably changed what it shows.
            post(move |app| {
                if let Some(r) = app.plugin_runner.borrow().as_ref() {
                    r.run_now(index);
                }
            });
        });
    }

    pub(super) fn toggle_plugin(&self, id: &str) {
        let mut disabled = self.cfg.borrow().str_list("plugins.disabled");
        if let Some(pos) = disabled.iter().position(|d| d == id) {
            disabled.remove(pos);
        } else {
            disabled.push(id.to_owned());
        }
        let result = {
            let mut cfg = self.cfg.borrow_mut();
            cfg.set("plugins.disabled", Value::from(disabled)).and_then(|_| cfg.save())
        };
        if let Err(e) = result {
            log::error!("saving plugins.disabled: {e:#}");
        }
        self.load_plugins();
    }
}
