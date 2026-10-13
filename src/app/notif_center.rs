//! Notification center (the sidebar drawer).

use super::*;

impl Caelestia {
    pub(super) fn on_notif_event(&self, ev: NotifEvent) {
        match ev {
            NotifEvent::Access(state) => {
                if state != AccessState::Allowed {
                    log::info!("notification access: {}", state.id());
                }
                each_window!(self, w => w.global::<ShellGlobal>().set_notif_access(state.id().into()));
            }
            NotifEvent::Icon(app_id, buf) => {
                self.st.borrow_mut().notif_icons.insert(app_id, Image::from_rgba8(buf));
                self.push_notifications();
            }
            NotifEvent::List(list) => {
                {
                    let mut st = self.st.borrow_mut();
                    // Forget the expanded state of apps that have nothing left.
                    st.notif_expanded.retain(|id| list.iter().any(|n| &n.app_id == id));
                    st.notifs = list;
                }
                self.push_notifications();
            }
        }
    }

    /// Groups -> the sidebar; the count -> every bar.
    pub(super) fn push_notifications(&self) {
        const COLLAPSED: usize = 3;
        let now = chrono::Utc::now().timestamp();
        let ago = |at: i64| crate::i18n::ago((now - at).max(0) as u64);
        let (groups, count) = {
            let st = self.st.borrow();
            let groups: Vec<NotifGroup> = notifications::group(&st.notifs)
                .into_iter()
                .map(|g| {
                    let expanded = st.notif_expanded.contains(&g.app_id);
                    let shown = if expanded { g.items.len() } else { g.items.len().min(COLLAPSED) };
                    let icon = st.notif_icons.get(&g.app_id).cloned();
                    NotifGroup {
                        time: ago(g.newest()).into(),
                        count: g.items.len() as i32,
                        more: (g.items.len() - shown) as i32,
                        expanded: expanded && g.items.len() > COLLAPSED,
                        has_icon: icon.is_some(),
                        icon: icon.unwrap_or_default(),
                        items: ModelRc::new(VecModel::from(
                            g.items
                                .iter()
                                .take(shown)
                                .map(|n| NotifItem {
                                    id: n.id as i32,
                                    title: if n.title.is_empty() { g.app.clone() } else { n.title.clone() }.into(),
                                    body: notification_body(&n.body).into(),
                                    time: ago(n.at).into(),
                                })
                                .collect::<Vec<_>>(),
                        )),
                        app_id: g.app_id.into(),
                        app: g.app.into(),
                    }
                })
                .collect();
            (groups, st.notifs.len() as i32)
        };
        self.sidebar.global::<ShellGlobal>().set_notif_groups(ModelRc::new(VecModel::from(groups)));
        for s in self.screens.borrow().iter() {
            s.bar.global::<ShellGlobal>().set_notif_count(count);
        }
        self.sidebar.global::<ShellGlobal>().set_notif_count(count);
    }

    /// Clicking a notification opens its app (like clicking the toast) and
    /// removes it from the list.
    pub(super) fn open_notification(&self, id: i32) {
        let app_id = self.st.borrow().notifs.iter().find(|n| n.id as i32 == id).map(|n| n.app_id.clone());
        let Some(app_id) = app_id else { return };
        self.close(Drawer::Sidebar);
        if !app_id.is_empty() {
            system::open(&format!("shell:AppsFolder\\{app_id}"));
        }
        self.dismiss_notifications(|n| n.id as i32 == id);
    }

    pub(super) fn dismiss_notifications(&self, which: impl Fn(&notifications::Notif) -> bool) {
        let ids: Vec<u32> = {
            let mut st = self.st.borrow_mut();
            let ids = st.notifs.iter().filter(|n| which(n)).map(|n| n.id).collect();
            st.notifs.retain(|n| !which(n));
            ids
        };
        if ids.is_empty() {
            return;
        }
        if let Some(svc) = self.svc.borrow().as_ref() {
            svc.notifications.send(NotifCommand::Remove(ids));
        }
        self.push_notifications();
    }
}
