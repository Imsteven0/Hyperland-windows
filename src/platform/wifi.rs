//! Wi-Fi networks through WinRT's `WiFiAdapter`: scan, list, connect (with a
//! password when needed) and disconnect. Every call blocks on WinRT async
//! operations: use from the connectivity thread.
//!
//! Since Windows 11 24H2, listing networks also needs location access for
//! desktop apps (Settings › Privacy › Location); without it the list is empty.

use windows::{
    Devices::WiFi::{WiFiAccessStatus, WiFiAdapter, WiFiAvailableNetwork, WiFiConnectionStatus, WiFiReconnectionKind},
    Networking::Connectivity::{NetworkAuthenticationType, NetworkInformation},
    Security::Credentials::PasswordCredential,
    core::HSTRING,
};

#[derive(Debug, Clone, PartialEq)]
pub struct WifiNetwork {
    pub ssid: String,
    /// 0..=4
    pub bars: u8,
    /// Needs a password (or enterprise credentials).
    pub secured: bool,
    pub connected: bool,
    /// Windows has a profile for it: connects without asking.
    pub saved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectError {
    Password,
    NotAvailable,
    Timeout,
    Unsupported,
    Failed,
}

impl ConnectError {
    pub fn message(self) -> &'static str {
        match self {
            ConnectError::Password => "Wrong password",
            ConnectError::NotAvailable => "Network not in range",
            ConnectError::Timeout => "Timed out",
            ConnectError::Unsupported => "Needs Windows settings (enterprise sign-in)",
            ConnectError::Failed => "Could not connect",
        }
    }
}

/// Asks Windows for Wi-Fi access (once); false when the user denied it.
pub fn request_access() -> bool {
    WiFiAdapter::RequestAccessAsync().and_then(|op| op.join()).map(|s| s == WiFiAccessStatus::Allowed).unwrap_or(false)
}

pub fn adapter() -> Option<WiFiAdapter> {
    let all = WiFiAdapter::FindAllAdaptersAsync().ok()?.join().ok()?;
    all.into_iter().next()
}

/// Refreshes the adapter's network report (takes a few seconds).
pub fn scan(adapter: &WiFiAdapter) -> bool {
    adapter.ScanAsync().and_then(|op| op.join()).is_ok()
}

fn is_secured(n: &WiFiAvailableNetwork) -> bool {
    let auth = n.SecuritySettings().and_then(|s| s.NetworkAuthenticationType()).unwrap_or(NetworkAuthenticationType::Unknown);
    !matches!(auth, NetworkAuthenticationType::None | NetworkAuthenticationType::Open80211 | NetworkAuthenticationType::Owe)
}

fn connected_ssid(adapter: &WiFiAdapter) -> Option<String> {
    let profile = adapter.NetworkAdapter().ok()?.GetConnectedProfileAsync().ok()?.join().ok()?;
    let ssid = profile.WlanConnectionProfileDetails().ok()?.GetConnectedSsid().ok()?.to_string();
    (!ssid.is_empty()).then_some(ssid)
}

fn saved_profiles() -> Vec<String> {
    let Ok(profiles) = NetworkInformation::GetConnectionProfiles() else { return Vec::new() };
    profiles.into_iter().filter(|p| p.IsWlanConnectionProfile().unwrap_or(false)).filter_map(|p| p.ProfileName().ok().map(|n| n.to_string())).collect()
}

/// Networks from the last scan: one row per name (strongest access point),
/// the connected one first, then saved ones, then by signal.
pub fn networks(adapter: &WiFiAdapter) -> Vec<WifiNetwork> {
    let Ok(report) = adapter.NetworkReport() else { return Vec::new() };
    let Ok(available) = report.AvailableNetworks() else { return Vec::new() };
    let connected = connected_ssid(adapter);
    let saved = saved_profiles();
    let mut out: Vec<WifiNetwork> = Vec::new();
    for n in available {
        let ssid = n.Ssid().map(|s| s.to_string()).unwrap_or_default();
        if ssid.is_empty() {
            continue; // hidden network
        }
        let bars = n.SignalBars().unwrap_or(0).min(4);
        match out.iter_mut().find(|w| w.ssid == ssid) {
            Some(w) => w.bars = w.bars.max(bars),
            None => out.push(WifiNetwork {
                connected: connected.as_deref() == Some(ssid.as_str()),
                saved: saved.iter().any(|s| s == &ssid),
                secured: is_secured(&n),
                ssid,
                bars,
            }),
        }
    }
    sort(&mut out);
    out
}

fn sort(networks: &mut [WifiNetwork]) {
    networks.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.saved.cmp(&a.saved)).then(b.bars.cmp(&a.bars)).then(a.ssid.to_lowercase().cmp(&b.ssid.to_lowercase())));
}

/// Connects to `ssid` (strongest access point of the last scan), with
/// `password` for a secured network Windows has no profile for.
pub fn connect(adapter: &WiFiAdapter, ssid: &str, password: Option<&str>) -> Result<(), ConnectError> {
    let report = adapter.NetworkReport().map_err(|_| ConnectError::Failed)?;
    let network = report
        .AvailableNetworks()
        .map_err(|_| ConnectError::Failed)?
        .into_iter()
        .filter(|n| n.Ssid().map(|s| s.to_string()).as_deref() == Ok(ssid))
        .max_by_key(|n| n.SignalBars().unwrap_or(0))
        .ok_or(ConnectError::NotAvailable)?;
    let result = match password {
        Some(pw) => {
            let cred = PasswordCredential::new().map_err(|_| ConnectError::Failed)?;
            cred.SetPassword(&HSTRING::from(pw)).map_err(|_| ConnectError::Failed)?;
            adapter.ConnectWithPasswordCredentialAsync(&network, WiFiReconnectionKind::Automatic, &cred)
        }
        None => adapter.ConnectAsync(&network, WiFiReconnectionKind::Automatic),
    }
    .and_then(|op| op.join())
    .and_then(|r| r.ConnectionStatus())
    .map_err(|_| ConnectError::Failed)?;
    match result {
        WiFiConnectionStatus::Success => Ok(()),
        WiFiConnectionStatus::InvalidCredential => Err(ConnectError::Password),
        WiFiConnectionStatus::NetworkNotAvailable => Err(ConnectError::NotAvailable),
        WiFiConnectionStatus::Timeout => Err(ConnectError::Timeout),
        WiFiConnectionStatus::UnsupportedAuthenticationProtocol => Err(ConnectError::Unsupported),
        _ => Err(ConnectError::Failed),
    }
}

pub fn disconnect(adapter: &WiFiAdapter) {
    if let Err(e) = adapter.Disconnect() {
        log::warn!("wifi: disconnect: {e}");
    }
}

// ---- without a scan: saved networks only ----
//
// With location services off, Windows hides the adapter and nearby networks
// from apps (netsh included), but saved profiles can still be listed and
// joined by name.

/// SSID of the current Wi-Fi connection, if any.
fn current_ssid() -> Option<String> {
    let profile = NetworkInformation::GetInternetConnectionProfile().ok()?;
    if !profile.IsWlanConnectionProfile().ok()? {
        return None;
    }
    profile.WlanConnectionProfileDetails().ok()?.GetConnectedSsid().ok().map(|s| s.to_string()).filter(|s| !s.is_empty())
}

/// Saved networks (no signal information), the connected one first.
pub fn saved_networks() -> Vec<WifiNetwork> {
    let connected = current_ssid();
    let mut names = saved_profiles();
    names.sort();
    names.dedup();
    let mut out: Vec<WifiNetwork> = names
        .into_iter()
        .map(|ssid| WifiNetwork { connected: connected.as_deref() == Some(ssid.as_str()), saved: true, secured: true, bars: 0, ssid })
        .collect();
    sort(&mut out);
    out
}

fn netsh(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("netsh")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Joins a saved network by its profile name.
pub fn connect_saved(ssid: &str) -> Result<(), ConnectError> {
    if netsh(&["wlan", "connect", &format!("name={ssid}")]) { Ok(()) } else { Err(ConnectError::Failed) }
}

pub fn disconnect_any() {
    if !netsh(&["wlan", "disconnect"]) {
        log::warn!("wifi: netsh disconnect failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(ssid: &str, bars: u8, connected: bool, saved: bool) -> WifiNetwork {
        WifiNetwork { ssid: ssid.into(), bars, secured: true, connected, saved }
    }

    #[test]
    fn connected_then_saved_then_strongest() {
        let mut list = vec![net("cafe", 4, false, false), net("home", 2, false, true), net("work", 1, true, true), net("Alpha", 4, false, false)];
        sort(&mut list);
        let order: Vec<&str> = list.iter().map(|n| n.ssid.as_str()).collect();
        assert_eq!(order, ["work", "home", "Alpha", "cafe"]);
    }
}
