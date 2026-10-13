//! Global shortcuts from `windows.shortcuts` in shell.json, e.g.
//! `"launcher": "Alt+Space"` or `"launcher": "Super"` (tap the Windows key,
//! like Caelestia's Super). Implemented with a low-level keyboard hook rather
//! than `RegisterHotKey`, so:
//!   * no other process can "own" our combinations (no registration races),
//!   * a bare Windows-key tap can replace the Start menu when configured.
//! The hook runs on the UI thread (it needs a message loop) and only matches
//! keys; actions are deferred to the event loop so the hook returns instantly.

use std::{cell::RefCell, rc::Rc};

use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    UI::{
        Input::KeyboardAndMouse::GetAsyncKeyState,
        WindowsAndMessaging::{CallNextHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP},
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
    on_press: Rc<dyn Fn(String)>,
    win_down: bool,
    win_used: bool,
}

thread_local! {
    static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

fn pressed(vk: u32) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

fn fire(target: String) {
    let Some(cb) = STATE.with(|s| s.borrow().as_ref().map(|s| s.on_press.clone())) else { return };
    // Never run app code inside the hook (it has a system-enforced timeout).
    slint::Timer::single_shot(std::time::Duration::ZERO, move || cb(target));
}

fn tap_dummy() {
    crate::platform::window::send_keys(&[], &[VK_DUMMY]);
}

unsafe extern "system" fn hook_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code != HC_ACTION as i32 {
            return CallNextHookEx(None, code, wp, lp);
        }
        let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        // Injected keys are honoured (AutoHotkey remaps, macro pads...), except
        // the dummy key we inject ourselves to cancel Start/menu activation.
        if k.flags.0 & LLKHF_INJECTED.0 != 0 && k.vkCode == VK_DUMMY as u32 {
            return CallNextHookEx(None, code, wp, lp);
        }
        let msg = wp.0 as u32;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        let vk = k.vkCode;
        let swallow = STATE.with(|s| {
            let mut s = s.borrow_mut();
            let Some(st) = s.as_mut() else { return false };
            if vk == VK_LWIN || vk == VK_RWIN {
                if down && !st.win_down {
                    st.win_down = true;
                    st.win_used = false;
                } else if up {
                    let tapped = st.win_down && !st.win_used && !pressed(VK_CONTROL) && !pressed(VK_MENU) && !pressed(VK_SHIFT);
                    st.win_down = false;
                    if tapped {
                        if let Some((_, t)) = st.binds.iter().find(|(c, _)| c.vk == 0 && c.win) {
                            // Cancel the Start menu, let the key-up through, open ours.
                            tap_dummy();
                            let t = t.clone();
                            drop(s);
                            fire(t);
                        }
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
            let t = t.clone();
            drop(s);
            fire(t);
            true
        });
        if swallow { LRESULT(1) } else { CallNextHookEx(None, code, wp, lp) }
    }
}

pub struct Hotkeys {
    hook: HHOOK,
}

impl Hotkeys {
    /// `on_press` receives the target name ("launcher", "dashboard", ...).
    /// Must be called on the UI thread.
    pub fn new(on_press: impl Fn(String) + 'static) -> Option<Self> {
        STATE.with(|s| *s.borrow_mut() = Some(HookState { binds: Vec::new(), on_press: Rc::new(on_press), win_down: false, win_used: false }));
        let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) }.map_err(|e| log::warn!("keyboard hook unavailable: {e}")).ok()?;
        Some(Self { hook })
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
        STATE.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                st.binds = parsed;
            }
        });
        errors
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.hook);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
