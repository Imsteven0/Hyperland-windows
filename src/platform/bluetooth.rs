//! Paired Bluetooth devices (classic + LE) via Windows.Devices.Enumeration,
//! with connection state, a type icon and unpairing. Audio devices can be
//! (dis)connected through `audio::bluetooth_audio_connect`.
//! All calls block on WinRT async operations: run them off the UI thread.

use windows_collections::IIterable;
use windows::{
    Devices::{
        Bluetooth::{BluetoothDevice, BluetoothLEDevice},
        Enumeration::DeviceInformation,
    },
    Foundation::IReference,
    core::{GUID, HSTRING, Interface},
};

const P_CONNECTED: &str = "System.Devices.Aep.IsConnected";
const P_MAJOR: &str = "System.Devices.Aep.Bluetooth.Cod.Major";
const P_CONTAINER: &str = "System.Devices.Aep.ContainerId";

#[derive(Debug, Clone, PartialEq)]
pub struct BtDevice {
    pub id: String,
    pub name: String,
    /// Material Symbols icon for the device type.
    pub icon: &'static str,
    pub connected: bool,
    /// Has an audio endpoint: we can connect/disconnect it on demand.
    pub audio: bool,
    pub container: Option<GUID>,
}

fn icon_for(major: u32, name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.contains("keyboard") || n.contains("teclado") {
        return "keyboard";
    }
    if n.contains("mouse") || n.contains("ratón") || n.contains("mx ") {
        return "mouse";
    }
    if n.contains("controller") || n.contains("gamepad") || n.contains("xbox") || n.contains("dualsense") || n.contains("dualshock") {
        return "sports_esports";
    }
    if n.contains("buds") || n.contains("airpods") || n.contains("earbuds") {
        return "earbuds";
    }
    match major {
        1 => "computer",
        2 => "smartphone",
        4 => {
            if ["speaker", "altavoz", "soundbar", "bass", "boom", "jbl", "flip", "charge"].iter().any(|k| n.contains(k)) { "speaker" } else { "headphones" }
        }
        5 => "mouse",
        6 => "print",
        7 => "watch",
        9 => "monitor_heart",
        _ => "bluetooth",
    }
}

fn prop<T>(info: &DeviceInformation, key: &str) -> Option<T>
where
    T: windows::core::RuntimeType + 'static,
{
    let props = info.Properties().ok()?;
    let v = props.Lookup(&HSTRING::from(key)).ok()?;
    v.cast::<IReference<T>>().ok()?.Value().ok()
}

/// Paired devices, classic and LE merged by container (one row per gadget).
pub fn paired_devices() -> Vec<BtDevice> {
    let mut out: Vec<BtDevice> = Vec::new();
    // Looked up once, lazily: enumerating audio endpoints per device was the
    // costliest part of the 3-second connectivity poll.
    let mut audio_containers: Option<std::collections::HashSet<GUID>> = None;
    let selectors = [BluetoothDevice::GetDeviceSelectorFromPairingState(true), BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)];
    for selector in selectors.into_iter().flatten() {
        let props: IIterable<HSTRING> = vec![HSTRING::from(P_CONNECTED), HSTRING::from(P_MAJOR), HSTRING::from(P_CONTAINER)].into();
        let Ok(coll) = DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(&selector, &props).and_then(|op| op.join()) else { continue };
        for info in coll {
            let name = info.Name().map(|n| n.to_string()).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let container = prop::<GUID>(&info, P_CONTAINER);
            let connected = prop::<bool>(&info, P_CONNECTED).unwrap_or(false);
            let major = prop::<u32>(&info, P_MAJOR)
                .or_else(|| prop::<u16>(&info, P_MAJOR).map(u32::from))
                .or_else(|| prop::<u8>(&info, P_MAJOR).map(u32::from))
                .unwrap_or(0);
            if let Some(existing) = out.iter_mut().find(|d| container.is_some() && d.container == container) {
                existing.connected |= connected;
                continue;
            }
            let audio = container.is_some_and(|c| audio_containers.get_or_insert_with(super::audio::audio_containers).contains(&c));
            // No class of device reported: an audio endpoint still tells us what it is.
            let major = if major == 0 && audio { 4 } else { major };
            out.push(BtDevice {
                id: info.Id().map(|i| i.to_string()).unwrap_or_default(),
                icon: icon_for(major, &name),
                name,
                connected,
                audio,
                container,
            });
        }
    }
    // AEP "IsConnected" lies for speakers paired twice (a stale entry stays
    // connected): for audio gadgets the active endpoint is the truth.
    for d in out.iter_mut().filter(|d| d.audio) {
        d.connected = d.container.is_some_and(super::audio::audio_endpoint_active);
    }
    // Connected first, then alphabetical.
    out.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// Every paired AEP entry as reported (for `caelestia diag`).
pub fn raw_entries() -> Vec<(&'static str, String, bool, Option<GUID>)> {
    let mut out = Vec::new();
    let selectors = [("classic", BluetoothDevice::GetDeviceSelectorFromPairingState(true)), ("le", BluetoothLEDevice::GetDeviceSelectorFromPairingState(true))];
    for (kind, selector) in selectors {
        let Ok(selector) = selector else { continue };
        let props: IIterable<HSTRING> = vec![HSTRING::from(P_CONNECTED), HSTRING::from(P_CONTAINER)].into();
        let Ok(coll) = DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(&selector, &props).and_then(|op| op.join()) else { continue };
        for info in coll {
            let name = info.Name().map(|n| n.to_string()).unwrap_or_default();
            out.push((kind, name, prop::<bool>(&info, P_CONNECTED).unwrap_or(false), prop::<GUID>(&info, P_CONTAINER)));
        }
    }
    out
}

/// Forgets (unpairs) a device.
pub fn unpair(id: &str) -> bool {
    DeviceInformation::CreateFromIdAsync(&HSTRING::from(id))
        .and_then(|op| op.join())
        .and_then(|info| info.Pairing())
        .and_then(|p| p.UnpairAsync())
        .and_then(|op| op.join())
        .is_ok()
}
