//! Core Audio: default render/capture volume + mute, endpoint lists, default
//! device switching, and Bluetooth audio (re)connection through the BT audio
//! driver's KS one-shot properties.

use std::ffi::c_void;

use windows::{
    Win32::{
        Devices::FunctionDiscovery::{PKEY_Device_ContainerId, PKEY_Device_FriendlyName},
        Media::{
            Audio::{
                DEVICE_STATE, DEVICE_STATE_ACTIVE, DEVICE_STATEMASK_ALL, EDataFlow, ERole, Endpoints::IAudioEndpointVolume, IDeviceTopology, IMMDevice, IMMDeviceEnumerator,
                IPart, MMDeviceEnumerator, eAll, eCapture, eCommunications, eConsole, eMultimedia, eRender,
            },
            KernelStreaming::{IKsControl, KSIDENTIFIER, KSIDENTIFIER_0, KSIDENTIFIER_0_0, KSPROPERTY_TYPE_GET},
        },
        System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ},
    },
    core::{GUID, HRESULT, IUnknown, Interface, PCWSTR, Result},
};

fn enumerator() -> Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

fn endpoint(flow: EDataFlow) -> Result<IAudioEndpointVolume> {
    unsafe {
        let device = enumerator()?.GetDefaultAudioEndpoint(flow, eConsole)?;
        device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub default: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioState {
    pub volume: f32,
    pub muted: bool,
    pub mic_volume: f32,
    pub mic_muted: bool,
}

pub fn state() -> AudioState {
    let mut s = AudioState::default();
    if let Ok(ep) = endpoint(eRender) {
        unsafe {
            s.volume = ep.GetMasterVolumeLevelScalar().unwrap_or(0.0);
            s.muted = ep.GetMute().map(|b| b.as_bool()).unwrap_or(false);
        }
    }
    if let Ok(ep) = endpoint(eCapture) {
        unsafe {
            s.mic_volume = ep.GetMasterVolumeLevelScalar().unwrap_or(0.0);
            s.mic_muted = ep.GetMute().map(|b| b.as_bool()).unwrap_or(false);
        }
    }
    s
}

fn set_level(flow: EDataFlow, v: f32) {
    if let Ok(ep) = endpoint(flow) {
        unsafe {
            let _ = ep.SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), &GUID::zeroed());
            if v > 0.0 {
                let _ = ep.SetMute(false, &GUID::zeroed());
            }
        }
    }
}

pub fn set_volume(v: f32) {
    set_level(eRender, v);
}

pub fn set_mic_volume(v: f32) {
    set_level(eCapture, v);
}

pub fn set_muted(capture: bool, muted: bool) {
    if let Ok(ep) = endpoint(if capture { eCapture } else { eRender }) {
        unsafe {
            let _ = ep.SetMute(muted, &GUID::zeroed());
        }
    }
}

fn device_id(d: &IMMDevice) -> Option<String> {
    unsafe {
        let p = d.GetId().ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }
}

fn friendly_name(d: &IMMDevice) -> String {
    unsafe {
        d.OpenPropertyStore(STGM_READ).and_then(|store| store.GetValue(&PKEY_Device_FriendlyName)).map(|v| v.to_string()).unwrap_or_default()
    }
}

/// Active endpoints of one direction, marking the current default.
pub fn devices(capture: bool) -> Vec<AudioDevice> {
    let flow = if capture { eCapture } else { eRender };
    let Ok(e) = enumerator() else { return Vec::new() };
    unsafe {
        let default = e.GetDefaultAudioEndpoint(flow, eConsole).ok().and_then(|d| device_id(&d));
        let Ok(coll) = e.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else { return Vec::new() };
        let n = coll.GetCount().unwrap_or(0);
        (0..n)
            .filter_map(|i| coll.Item(i).ok())
            .filter_map(|d| {
                let id = device_id(&d)?;
                Some(AudioDevice { default: Some(&id) == default.as_ref(), name: friendly_name(&d), id })
            })
            .collect()
    }
}

/// Makes `id` the default endpoint for all roles. Windows has no public API
/// for this; every audio switcher (EarTrumpet, SoundSwitch...) uses the
/// long-stable `IPolicyConfig` COM interface, as we do here.
pub fn set_default(id: &str) -> bool {
    const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
    const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
    type SetDefaultEndpoint = unsafe extern "system" fn(*mut c_void, PCWSTR, ERole) -> HRESULT;
    unsafe {
        let Ok(unk) = CoCreateInstance::<_, IUnknown>(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL) else { return false };
        let mut raw: *mut c_void = std::ptr::null_mut();
        if unk.query(&IID_POLICY_CONFIG, &mut raw).is_err() || raw.is_null() {
            return false;
        }
        // vtable: IUnknown(3), GetMixFormat, GetDeviceFormat, ResetDeviceFormat,
        // SetDeviceFormat, Get/SetProcessingPeriod, Get/SetShareMode,
        // Get/SetPropertyValue, SetDefaultEndpoint (index 13).
        let vtbl = *(raw as *const *const *const c_void);
        let set: SetDefaultEndpoint = std::mem::transmute(*vtbl.add(13));
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        let mut ok = true;
        for role in [eConsole, eMultimedia, eCommunications] {
            ok &= set(raw, PCWSTR(wide.as_ptr()), role).is_ok();
        }
        // Release the extra reference taken by QueryInterface.
        let _ = IUnknown::from_raw(raw);
        ok
    }
}

/// `IKsControl`s of the Bluetooth audio filters behind every endpoint of the
/// device `container`: endpoint -> topology connector -> connected part ->
/// that part's topology -> its device (the BT KS filter) -> IKsControl.
fn bt_ks_controls(container: GUID) -> Vec<IKsControl> {
    let Ok(e) = enumerator() else { return Vec::new() };
    let mut out = Vec::new();
    unsafe {
        let Ok(coll) = e.EnumAudioEndpoints(eAll, DEVICE_STATE(DEVICE_STATEMASK_ALL)) else { return out };
        for i in 0..coll.GetCount().unwrap_or(0) {
            let Ok(d) = coll.Item(i) else { continue };
            if endpoint_container(&d) != Some(container) {
                continue;
            }
            let ks = (|| -> Result<IKsControl> {
                let topo = d.Activate::<IDeviceTopology>(CLSCTX_ALL, None)?;
                let connector = topo.GetConnector(0)?;
                let part: IPart = connector.GetConnectedTo()?.cast()?;
                let filter_topo = part.GetTopologyObject()?;
                let id = filter_topo.GetDeviceId()?;
                let filter_id = id.to_string().unwrap_or_default();
                CoTaskMemFree(Some(id.0 as *const c_void));
                let filter = e.GetDevice(&windows::core::HSTRING::from(filter_id))?;
                filter.Activate::<IKsControl>(CLSCTX_ALL, None)
            })();
            match ks {
                Ok(k) => out.push(k),
                Err(err) => log::debug!("bt audio endpoint without KS control: {err}"),
            }
        }
    }
    out
}

/// How many BT KS filters we can reach for a device (diagnostics, no side effects).
pub fn bluetooth_audio_controls(container: GUID) -> usize {
    bt_ks_controls(container).len()
}

/// `KSPROPSETID_BtAudio` one-shot reconnect (0) / disconnect (1) on the
/// Bluetooth audio filters of device `container` — how audio devices are
/// (dis)connected without the Settings app.
pub fn bluetooth_audio_connect(container: GUID, connect: bool) -> bool {
    const KSPROPSETID_BTAUDIO: GUID = GUID::from_u128(0x7fa06c40_b8f6_4c7e_8556_e8c33a12e54d);
    let prop = KSIDENTIFIER {
        Anonymous: KSIDENTIFIER_0 { Anonymous: KSIDENTIFIER_0_0 { Set: KSPROPSETID_BTAUDIO, Id: if connect { 0 } else { 1 }, Flags: KSPROPERTY_TYPE_GET } },
    };
    let mut done = false;
    for ks in bt_ks_controls(container) {
        let mut returned = 0u32;
        match unsafe { ks.KsProperty(&prop, std::mem::size_of::<KSIDENTIFIER>() as u32, std::ptr::null_mut(), 0, &mut returned) } {
            Ok(()) => done = true,
            Err(e) => log::debug!("KS one-shot {} failed: {e}", if connect { "reconnect" } else { "disconnect" }),
        }
    }
    done
}

fn endpoint_container(d: &IMMDevice) -> Option<GUID> {
    unsafe {
        let v = d.OpenPropertyStore(STGM_READ).ok()?.GetValue(&PKEY_Device_ContainerId).ok()?;
        // VT_CLSID (72): the GUID is behind a pointer.
        if v.Anonymous.Anonymous.vt.0 != 72 {
            return None;
        }
        let p = v.Anonymous.Anonymous.Anonymous.puuid as *const GUID;
        (!p.is_null()).then(|| *p)
    }
}

/// Whether any audio endpoint (active or not) belongs to this container.
/// Whether the device has an *active* audio endpoint, i.e. its audio
/// profile is really connected (paired-but-idle endpoints are "unplugged").
pub fn audio_endpoint_active(container: GUID) -> bool {
    let Ok(e) = enumerator() else { return false };
    unsafe {
        let Ok(coll) = e.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE) else { return false };
        (0..coll.GetCount().unwrap_or(0)).filter_map(|i| coll.Item(i).ok()).any(|d| endpoint_container(&d) == Some(container))
    }
}

pub fn has_audio_endpoint(container: GUID) -> bool {
    let Ok(e) = enumerator() else { return false };
    unsafe {
        let Ok(coll) = e.EnumAudioEndpoints(eAll, DEVICE_STATE(DEVICE_STATEMASK_ALL)) else { return false };
        (0..coll.GetCount().unwrap_or(0)).filter_map(|i| coll.Item(i).ok()).any(|d| endpoint_container(&d) == Some(container))
    }
}
