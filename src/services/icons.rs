//! App icons for windows (bar workspaces). A window is resolved to an icon
//! key — its executable path, or the AppUserModelID for UWP apps hosted by
//! ApplicationFrameHost — and icons are extracted once on a worker thread
//! (shell image factory, high quality) then cached on the UI thread.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::Path,
    thread,
};

use crossbeam_channel::{Sender, unbounded};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, PROPERTYKEY},
        System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
        UI::{
            Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow},
            WindowsAndMessaging::GetWindowThreadProcessId,
        },
    },
    core::{GUID, PWSTR},
};

use crate::platform::shell_items::path_thumbnail;

/// PKEY_AppUserModel_ID
const PKEY_AUMID: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3), pid: 5 };

/// Icon key for a window: `aumid:<id>` for packaged apps, else the exe path.
pub fn window_key(hwnd: isize) -> Option<String> {
    let hwnd = HWND(hwnd as *mut _);
    let path = process_path(hwnd)?;
    let exe = Path::new(&path).file_name()?.to_string_lossy().to_ascii_lowercase();
    if exe == "applicationframehost.exe" || path.to_ascii_lowercase().contains("\\windowsapps\\") {
        if let Some(aumid) = window_aumid(hwnd) {
            return Some(format!("aumid:{aumid}"));
        }
    }
    Some(path)
}

fn process_path(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn window_aumid(hwnd: HWND) -> Option<String> {
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
        let value = store.GetValue(&PKEY_AUMID).ok()?;
        let s = value.to_string();
        (!s.is_empty()).then_some(s)
    }
}

/// Extracts the icon for a key (runs on the worker thread).
fn extract(key: &str, px: i32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let target = match key.strip_prefix("aumid:") {
        Some(aumid) => format!("shell:AppsFolder\\{aumid}"),
        None => key.to_owned(),
    };
    path_thumbnail(Path::new(&target), px, true)
}

pub struct IconCache {
    images: RefCell<HashMap<String, Option<Image>>>,
    pending: RefCell<HashSet<String>>,
    /// (hwnd, pid) -> icon key: resolving one opens the process, and the bar
    /// rebuilds its workspaces on every focus change.
    keys: RefCell<HashMap<(isize, u32), Option<String>>>,
    tx: Sender<String>,
}

impl IconCache {
    /// `on_ready(key, pixels)` is called from the worker; marshal it to the UI thread.
    pub fn new(px: i32, on_ready: impl Fn(String, Option<SharedPixelBuffer<Rgba8Pixel>>) + Send + 'static) -> Self {
        let (tx, rx) = unbounded::<String>();
        thread::Builder::new()
            .name("icons".into())
            .spawn(move || {
                crate::platform::com_init(true);
                for key in rx {
                    let buf = extract(&key, px);
                    on_ready(key, buf);
                }
            })
            .expect("spawn icon worker");
        Self { images: RefCell::new(HashMap::new()), pending: RefCell::new(HashSet::new()), keys: RefCell::new(HashMap::new()), tx }
    }

    /// `window_key`, cached per window. The pid is part of the cache key, so
    /// a reused HWND belonging to another process is resolved again.
    pub fn key_for(&self, hwnd: isize) -> Option<String> {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid)) };
        if pid == 0 {
            return None;
        }
        if let Some(k) = self.keys.borrow().get(&(hwnd, pid)) {
            return k.clone();
        }
        let key = window_key(hwnd);
        let mut keys = self.keys.borrow_mut();
        if keys.len() > 1024 {
            keys.clear(); // closed windows never leave: start over now and then
        }
        keys.insert((hwnd, pid), key.clone());
        key
    }

    /// Cached icon, or `None` while it's being extracted (requested once).
    pub fn get(&self, key: &str) -> Option<Image> {
        if let Some(img) = self.images.borrow().get(key) {
            return img.clone();
        }
        if self.pending.borrow_mut().insert(key.to_owned()) {
            let _ = self.tx.send(key.to_owned());
        }
        None
    }

    pub fn store(&self, key: String, buf: Option<SharedPixelBuffer<Rgba8Pixel>>) {
        self.pending.borrow_mut().remove(&key);
        self.images.borrow_mut().insert(key, buf.map(Image::from_rgba8_premultiplied));
    }
}
