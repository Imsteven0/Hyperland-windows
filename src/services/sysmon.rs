//! Resource monitor (dashboard "Performance" tab): CPU, GPU (PDH engine
//! counters + DXGI adapter name), memory, storage and network throughput.
//! Polls fast only while the dashboard is visible.

use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};
use windows::{
    Win32::{
        Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1},
        System::Performance::{
            PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
            PdhOpenQueryW,
        },
    },
    core::w,
};

#[derive(Debug, Clone, Default)]
pub struct SysSnapshot {
    pub cpu: f32,
    pub cpu_name: String,
    pub gpu: Option<f32>,
    pub gpu_name: String,
    pub cpu_temp: Option<f32>,
    /// Why the CPU temperature is missing: "install" | "start" | "" (ok).
    pub cpu_temp_help: &'static str,
    pub gpu_temp: Option<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_name: String,
    pub disk_used: u64,
    pub disk_total: u64,
    pub disk_count: usize,
    pub down_rate: f64,
    pub up_rate: f64,
    pub total_down: u64,
    pub total_up: u64,
    pub down_path: String,
    pub up_path: String,
}

pub struct SysMonitor {
    pub active: Arc<AtomicBool>,
    pub interval_ms: Arc<AtomicU64>,
    /// Bumped by the UI's disk picker; `u64::MAX` = system drive.
    pub disk_index: Arc<AtomicU64>,
    /// Wakes the sampler at once (dashboard opened, other disk picked).
    wake: crossbeam_channel::Sender<()>,
}

impl SysMonitor {
    pub fn spawn(on_update: impl Fn(SysSnapshot) + Send + 'static) -> Self {
        let active = Arc::new(AtomicBool::new(false));
        let interval_ms = Arc::new(AtomicU64::new(1000));
        let disk_index = Arc::new(AtomicU64::new(u64::MAX));
        let (a, iv, di) = (active.clone(), interval_ms.clone(), disk_index.clone());
        let (wake, woken) = crossbeam_channel::unbounded();
        thread::Builder::new()
            .name("sysmon".into())
            .spawn(move || run(a, iv, di, woken, on_update))
            .expect("spawn sysmon");
        Self { active, interval_ms, disk_index, wake }
    }

    /// Shows or hides the live stats; showing samples right away.
    pub fn set_active(&self, on: bool) {
        self.active.store(on, Ordering::Relaxed);
        let _ = self.wake.send(());
    }

    pub fn set_disk(&self, index: u64) {
        self.disk_index.store(index, Ordering::Relaxed);
        let _ = self.wake.send(());
    }
}

fn run(active: Arc<AtomicBool>, interval_ms: Arc<AtomicU64>, disk_index: Arc<AtomicU64>, woken: crossbeam_channel::Receiver<()>, on_update: impl Fn(SysSnapshot)) {
    let mut sys = System::new_with_specifics(RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing().with_cpu_usage()).with_memory(MemoryRefreshKind::nothing().with_ram()));
    let mut disks = Disks::new_with_refreshed_list();
    let mut nets = Networks::new_with_refreshed_list();
    let mut gpu = GpuCounter::new();
    let cpu_name = sys.cpus().first().map(|c| c.brand().trim().to_owned()).unwrap_or_default();
    let (gpu_name, gpu_luid) = gpu_adapter();
    let gpu_temp = gpu_luid.and_then(super::temps::GpuTemp::open);
    let mut cpu_temp = super::temps::CpuTemp::new();
    let mut temp_help: &'static str = "";
    let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let mut down_hist: VecDeque<f64> = VecDeque::from(vec![0.0; 30]);
    let mut up_hist: VecDeque<f64> = VecDeque::from(vec![0.0; 30]);
    let mut tick = 0u64;

    let mut last_sample = std::time::Instant::now();
    loop {
        let was_visible = active.load(Ordering::Relaxed);
        let disk_before = disk_index.load(Ordering::Relaxed);
        let interval = Duration::from_millis(interval_ms.load(Ordering::Relaxed).max(250));
        let deadline = last_sample + if was_visible { interval } else { interval * 4 };
        // Sleep until the next sample, or until opening the dashboard or
        // picking another disk asks for one now (no periodic wake-ups).
        loop {
            match woken.recv_deadline(deadline) {
                Ok(()) => {
                    let woke = (!was_visible && active.load(Ordering::Relaxed)) || disk_index.load(Ordering::Relaxed) != disk_before;
                    if woke {
                        // CPU usage is a delta between two refreshes: take the
                        // first one now so the shown value covers the next 200 ms,
                        // not the whole time the dashboard was closed.
                        if !was_visible {
                            sys.refresh_cpu_usage();
                        }
                        thread::sleep(Duration::from_millis(200).saturating_sub(if was_visible { last_sample.elapsed() } else { Duration::ZERO }));
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            }
        }
        let visible = active.load(Ordering::Relaxed);
        let elapsed = last_sample.elapsed();
        last_sample = std::time::Instant::now();
        tick += 1;

        // While hidden only the network history is kept up to date.
        if visible {
            sys.refresh_cpu_usage();
            sys.refresh_memory();
        }
        nets.refresh(true);
        if tick % 10 == 1 {
            disks.refresh(true);
        }
        let secs = elapsed.as_secs_f64().max(0.05);
        let (mut rx, mut tx, mut trx, mut ttx) = (0u64, 0u64, 0u64, 0u64);
        for (_, data) in nets.iter() {
            rx += data.received();
            tx += data.transmitted();
            trx += data.total_received();
            ttx += data.total_transmitted();
        }
        let (down, up) = (rx as f64 / secs, tx as f64 / secs);
        push(&mut down_hist, down);
        push(&mut up_hist, up);

        if !visible {
            continue; // keep history continuous but don't wake the UI
        }
        // Real volumes only, in drive-letter order; the picker cycles through them.
        let mut vols: Vec<_> = disks.iter().filter(|d| d.total_space() > 0).collect();
        vols.sort_by_key(|d| d.mount_point().to_path_buf());
        let wanted = disk_index.load(Ordering::Relaxed);
        let disk = if wanted == u64::MAX {
            vols.iter().find(|d| d.mount_point().to_string_lossy().starts_with(&system_drive)).or(vols.first()).copied()
        } else {
            vols.get(wanted as usize % vols.len().max(1)).copied()
        };
        let peak = down_hist.iter().chain(up_hist.iter()).cloned().fold(1024.0, f64::max);
        on_update(SysSnapshot {
            cpu: sys.global_cpu_usage() / 100.0,
            cpu_name: cpu_name.clone(),
            gpu: gpu.as_mut().and_then(GpuCounter::sample),
            gpu_name: gpu_name.clone(),
            cpu_temp: {
                let t = cpu_temp.read();
                // Re-check LibreHardwareMonitor's state every ~10 samples only.
                if t.is_some() {
                    temp_help = "";
                } else if tick % 10 == 1 || temp_help.is_empty() {
                    temp_help = match super::temps::lhm_path() {
                        None => "install",
                        Some(_) if super::temps::lhm_running() => "",
                        Some(_) => "start",
                    };
                }
                t
            },
            cpu_temp_help: temp_help,
            gpu_temp: gpu_temp.as_ref().and_then(|g| g.read()),
            mem_used: sys.used_memory(),
            mem_total: sys.total_memory(),
            disk_name: disk.map(|d| d.mount_point().to_string_lossy().trim_end_matches('\\').to_owned()).unwrap_or_default(),
            disk_used: disk.map(|d| d.total_space() - d.available_space()).unwrap_or(0),
            disk_total: disk.map(|d| d.total_space()).unwrap_or(0),
            disk_count: vols.len(),
            down_rate: down,
            up_rate: up,
            total_down: trx,
            total_up: ttx,
            down_path: sparkline(&down_hist, peak),
            up_path: sparkline(&up_hist, peak),
        });
    }
}

fn push(h: &mut VecDeque<f64>, v: f64) {
    h.pop_front();
    h.push_back(v);
}

/// SVG path in a 100x40 viewbox.
fn sparkline(h: &VecDeque<f64>, peak: f64) -> String {
    let n = h.len().max(2) - 1;
    let mut s = String::with_capacity(h.len() * 14);
    for (i, v) in h.iter().enumerate() {
        let x = i as f64 * 100.0 / n as f64;
        let y = 39.0 - (v / peak).clamp(0.0, 1.0) * 37.0;
        s.push_str(if i == 0 { "M " } else { " L " });
        s.push_str(&format!("{x:.1} {y:.1}"));
    }
    s
}

pub fn fmt_bytes(b: f64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}

pub fn fmt_gib(b: u64) -> String {
    format!("{:.1}", b as f64 / 1024f64.powi(3))
}

struct GpuCounter {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

// PDH handles are only used from the sysmon thread.
unsafe impl Send for GpuCounter {}

impl GpuCounter {
    fn new() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(None, 0, &mut query) != 0 {
                return None;
            }
            let mut counter = PDH_HCOUNTER::default();
            if PdhAddEnglishCounterW(query, w!(r"\GPU Engine(*engtype_3D)\Utilization Percentage"), 0, &mut counter) != 0 {
                return None;
            }
            PdhCollectQueryData(query);
            Some(Self { query, counter })
        }
    }

    fn sample(&mut self) -> Option<f32> {
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            let (mut size, mut count) = (0u32, 0u32);
            PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
            if size == 0 {
                return Some(0.0);
            }
            let mut buf = vec![0u8; size as usize];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
                return None;
            }
            let slice = std::slice::from_raw_parts(items, count as usize);
            let total: f64 = slice.iter().map(|i| i.FmtValue.Anonymous.doubleValue).sum();
            Some((total / 100.0).clamp(0.0, 1.0) as f32)
        }
    }
}

/// Name and LUID of the discrete (largest VRAM) GPU.
fn gpu_adapter() -> (String, Option<windows::Win32::Foundation::LUID>) {
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return (String::new(), None) };
        let mut best: Option<(u64, String, windows::Win32::Foundation::LUID)> = None;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(desc) = adapter.GetDesc1() else { continue };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let name = String::from_utf16_lossy(&desc.Description[..desc.Description.iter().position(|&c| c == 0).unwrap_or(0)]);
            let mem = desc.DedicatedVideoMemory as u64;
            if best.as_ref().is_none_or(|(m, _, _)| mem > *m) {
                best = Some((mem, name, desc.AdapterLuid));
            }
        }
        best.map(|(_, n, l)| (n, Some(l))).unwrap_or_default()
    }
}
