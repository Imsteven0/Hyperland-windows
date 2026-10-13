//! CPU / GPU temperatures without a kernel driver of our own.
//!
//! * GPU: the WDDM performance data the Task Manager uses
//!   (`D3DKMTQueryAdapterInfo(KMTQAITYPE_ADAPTERPERFDATA)`), so it works for
//!   NVIDIA, AMD and Intel alike.
//! * CPU: LibreHardwareMonitor / OpenHardwareMonitor sensors over WMI when one
//!   of them is running (exact Tctl / package temperature), otherwise the ACPI
//!   thermal zones exposed as performance counters.

use std::{collections::HashMap, ffi::c_void, mem::size_of};

use windows::{
    Wdk::Graphics::Direct3D::{
        D3DKMT_ADAPTER_PERFDATA, D3DKMT_CLOSEADAPTER, D3DKMT_OPENADAPTERFROMLUID, D3DKMT_QUERYADAPTERINFO, D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid,
        D3DKMTQueryAdapterInfo, KMTQAITYPE_ADAPTERPERFDATA,
    },
    Win32::{
        Foundation::LUID,
        System::Performance::{
            PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
            PdhOpenQueryW,
        },
    },
    core::w,
};
use wmi::{Variant, WMIConnection};

pub struct GpuTemp {
    adapter: u32,
}

impl GpuTemp {
    pub fn open(luid: LUID) -> Option<Self> {
        let mut open = D3DKMT_OPENADAPTERFROMLUID { AdapterLuid: luid, ..Default::default() };
        unsafe { D3DKMTOpenAdapterFromLuid(&mut open) }.ok().ok()?;
        let me = Self { adapter: open.hAdapter };
        me.read().is_some().then_some(me)
    }

    /// Degrees Celsius, or `None` when the driver doesn't report it.
    pub fn read(&self) -> Option<f32> {
        let mut data = D3DKMT_ADAPTER_PERFDATA::default();
        let mut q = D3DKMT_QUERYADAPTERINFO {
            hAdapter: self.adapter,
            Type: KMTQAITYPE_ADAPTERPERFDATA,
            pPrivateDriverData: &mut data as *mut _ as *mut c_void,
            PrivateDriverDataSize: size_of::<D3DKMT_ADAPTER_PERFDATA>() as u32,
        };
        unsafe { D3DKMTQueryAdapterInfo(&mut q) }.ok().ok()?;
        // Deci-degrees Celsius; 0 means "not reported".
        (data.Temperature > 0).then(|| data.Temperature as f32 / 10.0)
    }
}

impl Drop for GpuTemp {
    fn drop(&mut self) {
        let mut c = D3DKMT_CLOSEADAPTER { hAdapter: self.adapter };
        unsafe {
            let _ = D3DKMTCloseAdapter(&mut c);
        }
    }
}

pub struct CpuTemp {
    wmi: Option<WMIConnection>,
    zones: Option<(PDH_HQUERY, PDH_HCOUNTER)>,
}

impl CpuTemp {
    pub fn new() -> Self {
        Self { wmi: hardware_monitor(), zones: thermal_zones() }
    }

    pub fn read(&mut self) -> Option<f32> {
        if self.wmi.is_none() {
            // LibreHardwareMonitor may be started after us.
            self.wmi = hardware_monitor();
        }
        if let Some(t) = lhm_http() {
            return Some(t);
        }
        if let Some(conn) = &self.wmi {
            match conn.raw_query::<HashMap<String, Variant>>("SELECT Name, Identifier, Value FROM Sensor WHERE SensorType='Temperature'") {
                Ok(rows) => {
                    if let Some(t) = pick_cpu_sensor(&rows) {
                        return Some(t);
                    }
                }
                Err(_) => self.wmi = None,
            }
        }
        // ACPI thermal zones are often a fixed motherboard probe (e.g. a constant
        // 17-28 °C) rather than the CPU; showing such a value is worse than
        // "N/A", so only trust readings in a plausible CPU range.
        self.zones.and_then(|(q, c)| unsafe { max_counter(q, c) }).map(|k| k / 10.0 - 273.15).filter(|c| *c >= 30.0 && *c < 115.0)
    }
}

/// LibreHardwareMonitor's optional web server (Options › Remote Web Server).
fn lhm_http() -> Option<f32> {
    let agent = ureq::Agent::new_with_config(ureq::config::Config::builder().timeout_global(Some(std::time::Duration::from_millis(250))).build());
    let mut resp = agent.get("http://127.0.0.1:8085/data.json").call().ok()?;
    let v: serde_json::Value = resp.body_mut().read_json().ok()?;
    fn walk(n: &serde_json::Value, out: &mut Vec<(String, f32)>) {
        if let (Some(text), Some(value)) = (n["Text"].as_str(), n["Value"].as_str()) {
            if value.contains('°') {
                let num: String = value.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect();
                if let Ok(t) = num.replace(',', ".").parse::<f32>() {
                    out.push((text.to_owned(), t));
                }
            }
        }
        for c in n["Children"].as_array().into_iter().flatten() {
            walk(c, out);
        }
    }
    let mut temps = Vec::new();
    walk(&v, &mut temps);
    ["Core (Tctl/Tdie)", "CPU Package", "Tctl", "Package"].iter().find_map(|k| temps.iter().find(|(t, _)| t.contains(k)).map(|(_, v)| *v))
}

/// Where LibreHardwareMonitor is installed (winget portable or a manual copy).
pub fn lhm_path() -> Option<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Some(local) = dirs::data_local_dir() {
        roots.push(local.join(r"Microsoft\WinGet\Packages"));
        roots.push(local.join("Programs"));
    }
    roots.push(std::path::PathBuf::from(r"C:\Program Files"));
    for root in roots {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.contains("librehardwaremonitor") {
                let exe = e.path().join("LibreHardwareMonitor.exe");
                if exe.is_file() {
                    return Some(exe);
                }
            }
        }
    }
    None
}

pub fn lhm_running() -> bool {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    sys.processes_by_name(std::ffi::OsStr::new("LibreHardwareMonitor")).next().is_some()
}

fn hardware_monitor() -> Option<WMIConnection> {
    for ns in ["root\\LibreHardwareMonitor", "root\\OpenHardwareMonitor"] {
        if let Ok(conn) = WMIConnection::with_namespace_path(ns) {
            if conn.raw_query::<HashMap<String, Variant>>("SELECT Name FROM Sensor WHERE SensorType='Temperature'").is_ok_and(|r| !r.is_empty()) {
                log::info!("CPU temperature from {ns}");
                return Some(conn);
            }
        }
    }
    None
}

fn pick_cpu_sensor(rows: &[HashMap<String, Variant>]) -> Option<f32> {
    let text = |r: &HashMap<String, Variant>, k: &str| match r.get(k) {
        Some(Variant::String(s)) => s.clone(),
        _ => String::new(),
    };
    let value = |r: &HashMap<String, Variant>| match r.get("Value") {
        Some(Variant::R4(v)) => Some(*v),
        Some(Variant::R8(v)) => Some(*v as f32),
        _ => None,
    };
    let cpu: Vec<_> = rows.iter().filter(|r| text(r, "Identifier").contains("cpu")).collect();
    for preferred in ["Core (Tctl/Tdie)", "CPU Package", "Tctl", "Package", "Core Average"] {
        if let Some(v) = cpu.iter().find(|r| text(r, "Name").contains(preferred)).and_then(|r| value(r)) {
            return Some(v);
        }
    }
    cpu.iter().filter_map(|r| value(r)).fold(None, |m, v| Some(m.map_or(v, |m: f32| m.max(v))))
}

fn thermal_zones() -> Option<(PDH_HQUERY, PDH_HCOUNTER)> {
    unsafe {
        let mut q = PDH_HQUERY::default();
        if PdhOpenQueryW(None, 0, &mut q) != 0 {
            return None;
        }
        let mut c = PDH_HCOUNTER::default();
        if PdhAddEnglishCounterW(q, w!(r"\Thermal Zone Information(*)\High Precision Temperature"), 0, &mut c) != 0 {
            return None;
        }
        PdhCollectQueryData(q);
        Some((q, c))
    }
}

/// Max over all instances of a wildcard counter.
pub unsafe fn max_counter(q: PDH_HQUERY, c: PDH_HCOUNTER) -> Option<f32> {
    unsafe {
        if PdhCollectQueryData(q) != 0 {
            return None;
        }
        let (mut size, mut count) = (0u32, 0u32);
        PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, None);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
            return None;
        }
        std::slice::from_raw_parts(items, count as usize).iter().map(|i| i.FmtValue.Anonymous.doubleValue as f32).fold(None, |m, v| Some(m.map_or(v, |m: f32| m.max(v))))
    }
}
