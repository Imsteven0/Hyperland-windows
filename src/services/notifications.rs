//! Notification center: reads the notifications Windows keeps in its Action
//! Center through `UserNotificationListener`, like Caelestia's sidebar lists
//! the ones its daemon received. Needs "Notification access" (Settings ›
//! Privacy › Notifications), otherwise the sidebar explains how to allow it.
//!
//! Unpackaged apps can't subscribe to `NotificationChanged` on every Windows
//! build, so the worker uses the event when it can and otherwise polls (a
//! cheap local call), sending the list only when it actually changed.

use std::{
    collections::{HashMap, HashSet},
    thread,
    time::Duration,
};

use crossbeam_channel::{RecvTimeoutError, Sender, unbounded};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use windows::{
    Foundation::{Size, TypedEventHandler},
    Storage::Streams::DataReader,
    UI::Notifications::{
        KnownNotificationBindings,
        Management::{UserNotificationListener, UserNotificationListenerAccessStatus as Access},
        NotificationKinds, UserNotification, UserNotificationChangedEventArgs,
    },
};

#[derive(Debug, Clone, PartialEq)]
pub struct Notif {
    pub id: u32,
    /// AppUserModelID: opens the app (`shell:AppsFolder\<aumid>`) and keys its icon.
    pub app_id: String,
    pub app: String,
    pub title: String,
    pub body: String,
    /// Unix seconds.
    pub at: i64,
}

/// One app's notifications, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub app_id: String,
    pub app: String,
    pub items: Vec<Notif>,
}

impl Group {
    pub fn newest(&self) -> i64 {
        self.items.first().map(|n| n.at).unwrap_or(0)
    }
}

/// Groups by app; groups and their items are ordered newest first.
pub fn group(list: &[Notif]) -> Vec<Group> {
    let mut sorted: Vec<&Notif> = list.iter().collect();
    sorted.sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
    let mut groups: Vec<Group> = Vec::new();
    for n in sorted {
        match groups.iter_mut().find(|g| g.app_id == n.app_id) {
            Some(g) => g.items.push(n.clone()),
            None => groups.push(Group { app_id: n.app_id.clone(), app: n.app.clone(), items: vec![n.clone()] }),
        }
    }
    groups
}

/// A toast's text elements: the first is the title, the rest the body.
pub fn split_texts(texts: &[String]) -> (String, String) {
    let mut it = texts.iter().map(|t| t.trim()).filter(|t| !t.is_empty());
    let title = it.next().unwrap_or_default().to_owned();
    (title, it.collect::<Vec<_>>().join("\n"))
}

/// WinRT `DateTime` (100 ns ticks since 1601-01-01 UTC) to Unix seconds.
pub fn winrt_to_unix(ticks: i64) -> i64 {
    const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
    ticks / 10_000_000 - EPOCH_DIFF_SECS
}

/// Whether the shell may read notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessState {
    Allowed,
    /// The user (or policy) has not allowed it yet.
    Denied,
    /// This Windows can't give notifications to the shell.
    Unsupported,
}

impl AccessState {
    pub fn id(self) -> &'static str {
        match self {
            AccessState::Allowed => "allowed",
            AccessState::Denied => "denied",
            AccessState::Unsupported => "unsupported",
        }
    }
}

pub enum NotifEvent {
    Access(AccessState),
    List(Vec<Notif>),
    /// An app's logo, decoded once per app.
    Icon(String, SharedPixelBuffer<Rgba8Pixel>),
}

#[derive(Debug, Clone)]
pub enum NotifCommand {
    Remove(Vec<u32>),
    Clear,
    Refresh,
}

pub struct Notifications {
    tx: Sender<NotifCommand>,
}

impl Notifications {
    pub fn send(&self, cmd: NotifCommand) {
        let _ = self.tx.send(cmd);
    }

    pub fn spawn(on: impl Fn(NotifEvent) + Send + 'static) -> Self {
        let (tx, rx) = unbounded::<NotifCommand>();
        let wake = tx.clone();
        thread::Builder::new()
            .name("notifications".into())
            .spawn(move || {
                crate::platform::com_init(false);
                let listener = match UserNotificationListener::Current() {
                    Ok(l) => l,
                    Err(e) => {
                        log::warn!("notification listener unavailable: {e}");
                        on(NotifEvent::Access(AccessState::Unsupported));
                        return;
                    }
                };
                // Asking may show nothing for desktop apps; the status below decides.
                if listener.GetAccessStatus().is_ok_and(|s| s == Access::Unspecified) {
                    let _ = listener.RequestAccessAsync().and_then(|op| op.join());
                }
                let evented = listener
                    .NotificationChanged(&TypedEventHandler::<UserNotificationListener, UserNotificationChangedEventArgs>::new(move |_, _| {
                        let _ = wake.send(NotifCommand::Refresh);
                        Ok(())
                    }))
                    .is_ok();
                log::info!("notifications: {}", if evented { "change events" } else { "polling" });
                let poll = if evented { Duration::from_secs(30) } else { Duration::from_secs(2) };
                let mut access = None;
                let mut last: Option<Vec<Notif>> = None;
                let mut icons_done: HashSet<String> = HashSet::new();
                loop {
                    let state = match listener.GetAccessStatus() {
                        Ok(Access::Allowed) => AccessState::Allowed,
                        Ok(_) => AccessState::Denied,
                        Err(_) => AccessState::Unsupported,
                    };
                    if access != Some(state) {
                        access = Some(state);
                        on(NotifEvent::Access(state));
                    }
                    if state == AccessState::Allowed {
                        match read_all(&listener) {
                            Ok((list, infos)) => {
                                for (app_id, info) in infos {
                                    if icons_done.insert(app_id.clone()) {
                                        if let Some(buf) = logo(&info) {
                                            on(NotifEvent::Icon(app_id, buf));
                                        }
                                    }
                                }
                                if last.as_ref() != Some(&list) {
                                    last = Some(list.clone());
                                    on(NotifEvent::List(list));
                                }
                            }
                            Err(e) => log::debug!("reading notifications: {e}"),
                        }
                    }
                    // Wait for a command, a change event or the next poll;
                    // access changes are re-checked at least every 5 s.
                    let wait = if state == AccessState::Allowed { poll } else { Duration::from_secs(5) };
                    let first = match rx.recv_timeout(wait) {
                        Ok(cmd) => Some(cmd),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    };
                    // Handle everything queued meanwhile in one pass, so a burst
                    // of change events (a chat posting several) costs one read.
                    for cmd in first.into_iter().chain(rx.try_iter()) {
                        match cmd {
                            NotifCommand::Remove(ids) => ids.into_iter().for_each(|id| {
                                let _ = listener.RemoveNotification(id);
                            }),
                            NotifCommand::Clear => {
                                let _ = listener.ClearNotifications();
                            }
                            NotifCommand::Refresh => {}
                        }
                    }
                }
            })
            .expect("spawn notifications");
        Self { tx }
    }
}

type AppInfos = HashMap<String, windows::ApplicationModel::AppInfo>;

fn read_all(listener: &UserNotificationListener) -> windows::core::Result<(Vec<Notif>, AppInfos)> {
    let list = listener.GetNotificationsAsync(NotificationKinds::Toast)?.join()?;
    let mut out = Vec::new();
    let mut infos = AppInfos::new();
    for n in list {
        if let Some((notif, info)) = read_one(&n) {
            infos.entry(notif.app_id.clone()).or_insert(info);
            out.push(notif);
        }
    }
    out.sort_by_key(|n| n.id);
    Ok((out, infos))
}

fn read_one(n: &UserNotification) -> Option<(Notif, windows::ApplicationModel::AppInfo)> {
    let info = n.AppInfo().ok()?;
    let app_id = info.AppUserModelId().map(|s| s.to_string()).unwrap_or_default();
    let app = info.DisplayInfo().and_then(|d| d.DisplayName()).map(|s| s.to_string()).unwrap_or_else(|_| app_id.clone());
    let texts: Vec<String> = n
        .Notification()
        .and_then(|t| t.Visual())
        .and_then(|v| v.GetBinding(&KnownNotificationBindings::ToastGeneric()?))
        .and_then(|b| b.GetTextElements())
        .map(|els| els.into_iter().filter_map(|t| t.Text().ok()).map(|t| t.to_string()).collect())
        .unwrap_or_default();
    let (title, body) = split_texts(&texts);
    let at = n.CreationTime().map(|t| winrt_to_unix(t.UniversalTime)).unwrap_or(0);
    Some((Notif { id: n.Id().ok()?, app_id, app, title, body, at }, info))
}

fn logo(info: &windows::ApplicationModel::AppInfo) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let stream = info.DisplayInfo().ok()?.GetLogo(Size { Width: 64.0, Height: 64.0 }).ok()?.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > 4 * 1024 * 1024 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.join().ok()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.thumbnail(48, 48).into_rgba8();
    Some(SharedPixelBuffer::clone_from_slice(img.as_raw(), img.width(), img.height()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: u32, app: &str, at: i64) -> Notif {
        Notif { id, app_id: format!("{app}.id"), app: app.into(), title: format!("t{id}"), body: String::new(), at }
    }

    #[test]
    fn groups_by_app_newest_first() {
        let list = [n(1, "Mail", 100), n(2, "Chat", 300), n(3, "Mail", 400), n(4, "Chat", 200)];
        let g = group(&list);
        assert_eq!(g.iter().map(|g| g.app.as_str()).collect::<Vec<_>>(), ["Mail", "Chat"]);
        assert_eq!(g[0].items.iter().map(|n| n.id).collect::<Vec<_>>(), [3, 1]);
        assert_eq!(g[1].items.iter().map(|n| n.id).collect::<Vec<_>>(), [2, 4]);
        assert_eq!(g[0].newest(), 400);
        assert!(group(&[]).is_empty());
    }

    #[test]
    fn same_second_keeps_a_stable_order() {
        let g = group(&[n(7, "A", 50), n(9, "A", 50)]);
        assert_eq!(g[0].items.iter().map(|n| n.id).collect::<Vec<_>>(), [9, 7]);
    }

    #[test]
    fn splits_toast_texts() {
        let t = |v: &[&str]| split_texts(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(t(&["Ana", "Hola", "¿vienes?"]), ("Ana".into(), "Hola\n¿vienes?".into()));
        assert_eq!(t(&["", " Build ok "]), ("Build ok".into(), String::new()));
        assert_eq!(t(&[]), (String::new(), String::new()));
    }

    #[test]
    fn converts_winrt_time() {
        // 2026-01-01T00:00:00Z
        let ticks = (1_767_225_600 + 11_644_473_600) * 10_000_000;
        assert_eq!(winrt_to_unix(ticks), 1_767_225_600);
    }

    /// Needs a desktop session: reports access (and the list when allowed).
    #[test]
    #[ignore]
    fn service_reports_access() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let svc = Notifications::spawn(move |ev| {
            if let NotifEvent::Access(a) = ev {
                let _ = tx.send(a);
            }
        });
        let access = rx.recv_timeout(Duration::from_secs(10)).expect("access event");
        println!("notification access: {}", access.id());
        svc.send(NotifCommand::Refresh);
    }
}
