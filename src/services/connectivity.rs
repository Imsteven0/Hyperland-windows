//! Network status (WinRT NetworkInformation), Wi-Fi/Bluetooth radios and
//! paired Bluetooth devices. WinRT async calls block, so everything lives on
//! its own MTA thread.

use std::{thread, time::Duration};

use crossbeam_channel::{Sender, unbounded};
use windows::{
    Devices::Radios::{Radio, RadioKind, RadioState},
    Networking::Connectivity::{NetworkConnectivityLevel, NetworkInformation},
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetSnapshot {
    pub online: bool,
    pub wifi: bool,
    pub signal: i32,
    pub name: String,
    pub wifi_radio: Option<bool>,
    pub bt_radio: Option<bool>,
    pub bt_devices: Vec<crate::platform::bluetooth::BtDevice>,
}

#[derive(Debug, Clone)]
pub enum RadioCmd {
    ToggleWifi,
    ToggleBluetooth,
    /// Connect (true) / disconnect (false) a paired Bluetooth audio device by id.
    BtConnect(String, bool),
    BtForget(String),
    Refresh,
}

pub struct Connectivity {
    tx: Sender<RadioCmd>,
}

impl Connectivity {
    pub fn spawn(on_update: impl Fn(NetSnapshot) + Send + 'static) -> Self {
        let (tx, rx) = unbounded();
        thread::Builder::new()
            .name("connectivity".into())
            .spawn(move || {
                crate::platform::com_init(false);
                let _ = Radio::RequestAccessAsync().and_then(|op| op.join());
                let mut last: Option<NetSnapshot> = None;
                let mut timeout = Duration::from_secs(3);
                loop {
                    match rx.recv_timeout(timeout) {
                        Ok(RadioCmd::ToggleWifi) => toggle(RadioKind::WiFi),
                        Ok(RadioCmd::ToggleBluetooth) => toggle(RadioKind::Bluetooth),
                        Ok(RadioCmd::BtConnect(id, connect)) => {
                            let container = last.as_ref().and_then(|s| s.bt_devices.iter().find(|d| d.id == id)).and_then(|d| d.container);
                            match container {
                                Some(c) if crate::platform::audio::bluetooth_audio_connect(c, connect) => {}
                                _ => log::warn!("bluetooth: could not {} {id}", if connect { "connect" } else { "disconnect" }),
                            }
                        }
                        Ok(RadioCmd::BtForget(id)) => {
                            if !crate::platform::bluetooth::unpair(&id) {
                                log::warn!("bluetooth: could not unpair {id}");
                            }
                        }
                        Ok(RadioCmd::Refresh) => last = None,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                        Err(_) => {}
                    }
                    let snap = snapshot();
                    // (Dis)connecting takes a few seconds: poll faster meanwhile.
                    timeout = if last.as_ref().is_some_and(|l| l.bt_devices != snap.bt_devices) { Duration::from_secs(1) } else { Duration::from_secs(3) };
                    if last.as_ref() != Some(&snap) {
                        last = Some(snap.clone());
                        on_update(snap);
                    }
                }
            })
            .expect("spawn connectivity");
        Self { tx }
    }

    pub fn send(&self, cmd: RadioCmd) {
        let _ = self.tx.send(cmd);
    }
}

fn radio_state(kind: RadioKind) -> Option<bool> {
    let radios = Radio::GetRadiosAsync().ok()?.join().ok()?;
    let mut found = None;
    for r in radios {
        if r.Kind().ok()? == kind {
            let on = r.State().ok()? == RadioState::On;
            found = Some(found.unwrap_or(false) || on);
        }
    }
    found
}

fn toggle(kind: RadioKind) {
    let Ok(radios) = Radio::GetRadiosAsync().and_then(|op| op.join()) else { return };
    for r in radios {
        if r.Kind().ok() == Some(kind) {
            let next = if r.State().ok() == Some(RadioState::On) { RadioState::Off } else { RadioState::On };
            if let Err(e) = r.SetStateAsync(next).and_then(|op| op.join()) {
                log::warn!("radio toggle failed: {e}");
            }
        }
    }
}

fn snapshot() -> NetSnapshot {
    let mut s = NetSnapshot { wifi_radio: radio_state(RadioKind::WiFi), bt_radio: radio_state(RadioKind::Bluetooth), ..Default::default() };
    if s.bt_radio != Some(false) {
        s.bt_devices = crate::platform::bluetooth::paired_devices();
    }
    if let Ok(profile) = NetworkInformation::GetInternetConnectionProfile() {
        s.online = profile.GetNetworkConnectivityLevel().map(|l| l == NetworkConnectivityLevel::InternetAccess).unwrap_or(false);
        s.wifi = profile.IsWlanConnectionProfile().unwrap_or(false);
        s.signal = profile.GetSignalBars().ok().and_then(|r| r.Value().ok()).map(|v| v as i32).unwrap_or(if s.online { 4 } else { 0 });
        s.name = profile.ProfileName().map(|n| n.to_string()).unwrap_or_default();
    }
    s
}
