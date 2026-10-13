//! Global shortcuts from `windows.shortcuts` in shell.json, e.g.
//! `"launcher": "Alt+Space"` or `"launcher": "Super"` (tap the Windows key,
//! like Caelestia's Super). Implemented with a low-level keyboard hook rather
//! than `RegisterHotKey`, so:
//!   * no other process can "own" our combinations (no registration races),
//!   * a bare Windows-key tap can replace the Start menu when configured.
//!
//! The hook runs on a thread of its own with just a message loop: Windows
//! waits for it on every key press system-wide, so it must never queue behind
//! UI work (rendering, searches). It only matches keys; actions go to the UI.

use std::{
    sync::{Mutex, MutexGuard},
    thread::JoinHandle,
};

use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    UI::{
        Input::KeyboardAndMouse::GetAsyncKeyState,
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
            WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
/// Unassigned virtual key used to "consume" a modifier tap so Windows doesn't
/// open the Start menu (Win) or focus a menu bar (Alt).
const VK_DUMMY: u16 = 0xE8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Chord {
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
    /// 0 = modifier-only tap (only "Super" is supported).
    vk: u32,
}

fn parse(accel: &str) -> Result<Chord, String> {
    let mut c = Chord { ctrl: false, alt: false, shift: false, win: false, vk: 0 };
    for part in accel.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => c.ctrl = true,
            "alt" => c.alt = true,
            "shift" => c.shift = true,
            "super" | "win" | "meta" | "cmd" => c.win = true,
            key => c.vk = key_code(key).ok_or_else(|| format!("unknown key '{part}'"))?,
        }
    }
    if c.vk == 0 && !(c.win && !c.ctrl && !c.alt && !c.shift) {
        return Err("a shortcut needs a key (only \"Super\" works alone)".into());
    }
    Ok(c)
}

fn key_code(k: &str) -> Option<u32> {
    // Accept both "D" and the W3C/global-hotkey style "KeyD"/"Digit1".
    let k = k.strip_prefix("key").or_else(|| k.strip_prefix("digit")).unwrap_or(k);
    if k.len() == 1 {
        let ch = k.chars().next()?.to_ascii_uppercase();
        if ch.is_ascii_alphanumeric() {
            return Some(ch as u32);
        }
    }
    if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=24).contains(n)) {
        return Some(0x70 + n - 1);
    }
    Some(match k {
        "space" => 0x20,
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "escape" | "esc" => 0x1B,
        "backspace" => 0x08,
        "delete" => 0x2E,
        "insert" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "left" | "arrowleft" => 0x25,
        "up" | "arrowup" => 0x26,
        "right" | "arrowright" => 0x27,
        "down" | "arrowdown" => 0x28,
        "printscreen" => 0x2C,
        "comma" => 0xBC,
        "plus" | "equal" => 0xBB,
        "minus" => 0xBD,
        "period" => 0xBE,
        "slash" => 0xBF,
        "backquote" | "grave" => 0xC0,
        _ => return None,
    })
}

struct HookState {
    binds: Vec<(Chord, String)>,
    on_press: Box<dyn Fn(String) + Send>,
    win_down: bool,
    win_used: bool,
}

/// Shared by the hook thread and `bind` (UI thread); only held for a lookup.
static STATE: Mutex<Option<HookState>> = Mutex::new(None);

fn state() -> MutexGuard<'static, Option<HookState>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn pressed(vk: u32) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

fn tap_dummy() {
    crate::platform::window::send_keys(&[], &[VK_DUMMY]);
}

unsafe extern "system" fn hook_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code == HC_ACTION as i32 {
            let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
            let msg = wp.0 as u32;
            // Injected keys are honoured (AutoHotkey remaps, macro pads...), except
            // the dummy key we inject ourselves to cancel Start/menu activation.
            let ours = k.flags.0 & LLKHF_INJECTED.0 != 0 && k.vkCode == VK_DUMMY as u32;
            if !ours && on_key(k.vkCode, msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN, msg == WM_KEYUP || msg == WM_SYSKEYUP) {
                return LRESULT(1);
            }
        }
        CallNextHookEx(None, code, wp, lp)
    }
}

/// Matches one key event against the binds; true swallows it.
fn on_key(vk: u32, down: bool, up: bool) -> bool {
    let mut s = state();
    let Some(st) = s.as_mut() else { return false };
    if vk == VK_LWIN || vk == VK_RWIN {
        if down && !st.win_down {
            st.win_down = true;
            st.win_used = false;
        } else if up {
            let tapped = st.win_down && !st.win_used && !pressed(VK_CONTROL) && !pressed(VK_MENU) && !pressed(VK_SHIFT);
            st.win_down = false;
            if tapped && let Some((_, t)) = st.binds.iter().find(|(c, _)| c.vk == 0 && c.win) {
                // Cancel the Start menu, let the key-up through, open ours.
                tap_dummy();
                (st.on_press)(t.clone());
            }
        }
        return false;
    }
    if !down {
        return false;
    }
    if st.win_down {
        st.win_used = true;
    }
    let chord = Chord {
        ctrl: pressed(VK_CONTROL),
        alt: pressed(VK_MENU),
        shift: pressed(VK_SHIFT),
        win: st.win_down || pressed(VK_LWIN) || pressed(VK_RWIN),
        vk,
    };
    let Some((c, t)) = st.binds.iter().find(|(c, _)| c.vk != 0 && *c == chord) else { return false };
    if c.alt || c.win {
        tap_dummy(); // keep Alt/Win release from activating menus/Start
    }
    // Only queues the action on the UI thread: the hook returns at once.
    (st.on_press)(t.clone());
    true
}

/// The bound target for a key pressed in one of our own windows. Windows does
/// not run our low-level hook while a window of this process is in the
/// foreground (seen with the launcher and Settings), so those windows pass
/// their key presses here instead. `text` is Slint's `KeyEvent.text`.
pub fn local_target(text: &str, ctrl: bool, alt: bool, shift: bool, win: bool) -> Option<String> {
    let chord = Chord { ctrl, alt, shift, win, vk: vk_of(text, ctrl)? };
    state().as_ref()?.binds.iter().find(|(c, _)| c.vk != 0 && *c == chord).map(|(_, t)| t.clone())
}

/// Virtual-key code of a Slint key text (what `key_code` accepts).
fn vk_of(text: &str, ctrl: bool) -> Option<u32> {
    use slint::platform::Key;
    let mut chars = text.chars();
    let (ch, None) = (chars.next()?, chars.next()) else { return None };
    // Ctrl+letter may arrive as its control character (Ctrl+S = U+0013).
    let ch = if ctrl && ('\u{1}'..='\u{1a}').contains(&ch) { char::from(ch as u8 + 0x40) } else { ch };
    if ch.is_ascii_alphanumeric() {
        return Some(ch.to_ascii_uppercase() as u32);
    }
    let special: [(char, u32); 15] = [
        (Key::Space.into(), 0x20),
        (Key::Tab.into(), 0x09),
        (Key::Return.into(), 0x0D),
        (Key::Escape.into(), 0x1B),
        (Key::Backspace.into(), 0x08),
        (Key::Delete.into(), 0x2E),
        (Key::Insert.into(), 0x2D),
        (Key::Home.into(), 0x24),
        (Key::End.into(), 0x23),
        (Key::PageUp.into(), 0x21),
        (Key::PageDown.into(), 0x22),
        (Key::LeftArrow.into(), 0x25),
        (Key::UpArrow.into(), 0x26),
        (Key::RightArrow.into(), 0x27),
        (Key::DownArrow.into(), 0x28),
    ];
    if let Some((_, vk)) = special.iter().find(|(c, _)| *c == ch) {
        return Some(*vk);
    }
    let f1: char = Key::F1.into();
    if let Some(n) = (ch as u32).checked_sub(f1 as u32).filter(|n| *n < 24) {
        return Some(0x70 + n);
    }
    Some(match ch {
        ',' => 0xBC,
        '=' | '+' => 0xBB,
        '-' => 0xBD,
        '.' => 0xBE,
        '/' => 0xBF,
        '`' => 0xC0,
        _ => return None,
    })
}

pub struct Hotkeys {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl Hotkeys {
    /// `on_press` receives the target name ("launcher", "dashboard", ...) on
    /// the hook thread: it must only hand it over (e.g. post to the UI loop).
    pub fn new(on_press: impl Fn(String) + Send + 'static) -> Option<Self> {
        *state() = Some(HookState { binds: Vec::new(), on_press: Box::new(on_press), win_down: false, win_used: false });
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("hotkeys".into())
            .spawn(move || unsafe {
                use windows::Win32::System::Threading::GetCurrentThreadId;
                let hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) {
                    Ok(h) => h,
                    Err(e) => {
                        log::warn!("keyboard hook unavailable: {e}");
                        let _ = tx.send(None);
                        return;
                    }
                };
                let _ = tx.send(Some(GetCurrentThreadId()));
                // The hook is called from this loop; WM_QUIT (from Drop) ends it.
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                let _ = UnhookWindowsHookEx(hook);
            })
            .ok()?;
        match rx.recv().ok().flatten() {
            Some(thread_id) => Some(Self { thread_id, thread: Some(thread) }),
            None => {
                let _ = thread.join();
                *state() = None;
                None
            }
        }
    }

    /// `binds` are `(target, accelerator)` pairs. Returns the invalid ones.
    pub fn bind(&mut self, binds: &[(String, String)]) -> Vec<String> {
        let mut errors = Vec::new();
        let parsed: Vec<(Chord, String)> = binds
            .iter()
            .filter_map(|(target, accel)| match parse(accel) {
                Ok(c) => Some((c, target.clone())),
                Err(e) => {
                    log::warn!("invalid shortcut '{accel}' for {target}: {e}");
                    errors.push(format!("{target}: {e}"));
                    None
                }
            })
            .collect();
        if let Some(st) = state().as_mut() {
            st.binds = parsed;
        }
        errors
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        *state() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slint_keys_map_like_the_hook() {
        use slint::platform::Key;
        assert_eq!(vk_of(" ", false), Some(0x20));
        assert_eq!(vk_of("s", false), Some('S' as u32));
        assert_eq!(vk_of("\u{13}", true), Some('S' as u32), "Ctrl+S as a control character");
        assert_eq!(vk_of(&char::from(Key::F5).to_string(), false), Some(0x74));
        assert_eq!(vk_of(&char::from(Key::LeftArrow).to_string(), false), Some(0x25));
        assert_eq!(vk_of("ab", false), None);
        assert_eq!(vk_of("", false), None);
        // Same codes as the accelerators parse to.
        assert_eq!(vk_of(" ", false), Some(parse("Alt+Space").unwrap().vk));
        assert_eq!(vk_of("d", true), Some(parse("Ctrl+Alt+KeyD").unwrap().vk));
    }

    #[test]
    fn parses_common_accelerators() {
        assert_eq!(parse("Alt+Space").unwrap(), Chord { ctrl: false, alt: true, shift: false, win: false, vk: 0x20 });
        assert_eq!(parse("Ctrl+Alt+KeyD").unwrap().vk, 'D' as u32);
        assert_eq!(parse("Super+Shift+F5").unwrap(), Chord { ctrl: false, alt: false, shift: true, win: true, vk: 0x74 });
        assert!(parse("Super").unwrap().win);
        assert!(parse("Ctrl").is_err());
        assert!(parse("Alt+Nope").is_err());
    }
}
