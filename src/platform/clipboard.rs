//! Clipboard text: writing (calculator/emoji/history results) and watching
//! for new copies (launcher history).
//!
//! The watcher is a message-only window on its own thread registered with
//! `AddClipboardFormatListener` — no polling. Copies that ask not to be
//! recorded are skipped, the same way Windows' own clipboard history does:
//! password managers (KeePass, Bitwarden, 1Password...) set
//! `ExcludeClipboardContentFromMonitorProcessing` or
//! `CanIncludeInClipboardHistory = 0` on secrets.

use std::{cell::RefCell, thread, time::Duration};

use windows::{
    Win32::{
        Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM},
        System::{
            DataExchange::{AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, RemoveClipboardFormatListener, SetClipboardData},
            LibraryLoader::GetModuleHandleW,
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
            Ole::CF_UNICODETEXT,
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG, PostMessageW, PostQuitMessage, RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE, WM_CLOSE, WM_DESTROY,
            WNDCLASSW,
        },
    },
    core::{PCWSTR, w},
};

const CF_TEXT: u32 = CF_UNICODETEXT.0 as u32;

/// Opens the clipboard, retrying briefly: another process may hold it for a
/// few milliseconds right after it changes.
fn open(owner: Option<HWND>) -> bool {
    for attempt in 0..10 {
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(5 * (attempt + 1)));
    }
    false
}

/// Puts `text` on the clipboard.
pub fn set_text(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    if !open(None) {
        return false;
    }
    let ok = unsafe {
        let _ = EmptyClipboard();
        match GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            Ok(mem) => {
                let dst = GlobalLock(mem) as *mut u16;
                if dst.is_null() {
                    let _ = GlobalFree(Some(mem));
                    false
                } else {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
                    let _ = GlobalUnlock(mem);
                    // On success the clipboard owns the memory.
                    match SetClipboardData(CF_TEXT, Some(HANDLE(mem.0))) {
                        Ok(_) => true,
                        Err(_) => {
                            let _ = GlobalFree(Some(mem));
                            false
                        }
                    }
                }
            }
            Err(_) => false,
        }
    };
    unsafe {
        let _ = CloseClipboard();
    }
    ok
}

fn format_id(name: PCWSTR) -> u32 {
    unsafe { RegisterClipboardFormatW(name) }
}

/// Whether the current clipboard content asked not to be recorded.
/// Must be called with the clipboard open.
fn is_private() -> bool {
    unsafe {
        let exclude = format_id(w!("ExcludeClipboardContentFromMonitorProcessing"));
        let ignore = format_id(w!("Clipboard Viewer Ignore"));
        if [exclude, ignore].into_iter().any(|f| f != 0 && IsClipboardFormatAvailable(f).is_ok()) {
            return true;
        }
        let history = format_id(w!("CanIncludeInClipboardHistory"));
        if history == 0 || IsClipboardFormatAvailable(history).is_err() {
            return false;
        }
        let Ok(h) = GetClipboardData(history) else { return false };
        let mem = HGLOBAL(h.0);
        let p = GlobalLock(mem) as *const u32;
        let allowed = p.is_null() || GlobalSize(mem) < 4 || *p != 0;
        let _ = GlobalUnlock(mem);
        !allowed
    }
}

/// Reads the clipboard's text unless it is private. Clipboard must be open.
fn read_text() -> Option<String> {
    unsafe {
        if is_private() || IsClipboardFormatAvailable(CF_TEXT).is_err() {
            return None;
        }
        let h = GetClipboardData(CF_TEXT).ok()?;
        let mem = HGLOBAL(h.0);
        let p = GlobalLock(mem) as *const u16;
        if p.is_null() {
            return None;
        }
        let max = GlobalSize(mem) / 2;
        let len = (0..max).find(|&i| *p.add(i) == 0).unwrap_or(max);
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        let _ = GlobalUnlock(mem);
        Some(text)
    }
}

type OnText = Box<dyn Fn(String)>;

thread_local! {
    static ON_TEXT: RefCell<Option<OnText>> = const { RefCell::new(None) };
}

unsafe extern "system" fn watcher_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            if open(Some(hwnd)) {
                let text = read_text();
                unsafe {
                    let _ = CloseClipboard();
                }
                if let Some(text) = text {
                    ON_TEXT.with(|f| {
                        if let Some(f) = f.borrow().as_ref() {
                            f(text);
                        }
                    });
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = RemoveClipboardFormatListener(hwnd);
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

/// Watches the clipboard until dropped.
pub struct ClipboardWatcher {
    hwnd: isize,
}

impl Drop for ClipboardWatcher {
    fn drop(&mut self) {
        unsafe {
            let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

/// Calls `on_text` (on the watcher thread) for every new text copy.
pub fn watch(on_text: impl Fn(String) + Send + 'static) -> Option<ClipboardWatcher> {
    let (tx, rx) = crossbeam_channel::bounded::<Option<isize>>(1);
    thread::Builder::new()
        .name("clipboard".into())
        .spawn(move || unsafe {
            let class = w!("CaelestiaClipboardWatcher");
            let instance = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(watcher_proc), hInstance: instance, lpszClassName: class, ..Default::default() });
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("caelestia-clipboard"), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(instance), None);
            let Ok(hwnd) = hwnd else {
                let _ = tx.send(None);
                return;
            };
            if AddClipboardFormatListener(hwnd).is_err() {
                let _ = DestroyWindow(hwnd);
                let _ = tx.send(None);
                return;
            }
            ON_TEXT.with(|f| *f.borrow_mut() = Some(Box::new(on_text)));
            let _ = tx.send(Some(hwnd.0 as isize));
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&msg);
            }
        })
        .ok()?;
    let hwnd = rx.recv().ok().flatten()?;
    Some(ClipboardWatcher { hwnd })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a desktop session: `cargo test -- --ignored clipboard`.
    #[test]
    #[ignore]
    fn clipboard_roundtrip_and_watcher() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let watcher = watch(move |t| {
            let _ = tx.send(t);
        })
        .expect("watcher");
        assert!(set_text("hola ñandú 🔥"));
        assert!(open(None));
        let read = read_text();
        unsafe {
            let _ = CloseClipboard();
        }
        assert_eq!(read.as_deref(), Some("hola ñandú 🔥"));
        let seen = rx.recv_timeout(Duration::from_secs(5)).expect("clipboard update");
        assert_eq!(seen, "hola ñandú 🔥");
        drop(watcher);
    }

    /// Like a password manager: text plus `CanIncludeInClipboardHistory = 0`.
    #[test]
    #[ignore]
    fn private_copies_are_skipped() {
        unsafe {
            assert!(open(None));
            let _ = EmptyClipboard();
            for (format, bytes) in [(CF_TEXT, "s3cret\0".encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>()), (format_id(w!("CanIncludeInClipboardHistory")), 0u32.to_le_bytes().to_vec())] {
                let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).unwrap();
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), GlobalLock(mem) as *mut u8, bytes.len());
                let _ = GlobalUnlock(mem);
                SetClipboardData(format, Some(HANDLE(mem.0))).unwrap();
            }
            let _ = CloseClipboard();
            assert!(open(None));
            let private = is_private();
            let read = read_text();
            let _ = CloseClipboard();
            assert!(private);
            assert_eq!(read, None);
        }
    }
}
