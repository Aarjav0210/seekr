//! The global hotkey, registered as a Carbon hot key: macOS delivers it to
//! this app whichever app is in front, with no Accessibility or Input
//! Monitoring permission (a key event tap would need one). It is registered
//! shared rather than exclusive: an exclusive registration fails while any
//! other app holds the same combination, even one that never reacts to it.
use std::ffi::c_void;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    /// Virtual key code (Carbon's kVK_*).
    pub key: u32,
    /// Carbon modifier mask (cmdKey, shiftKey, optionKey, controlKey).
    pub modifiers: u32,
}

const CMD: u32 = 1 << 8;
const SHIFT: u32 = 1 << 9;
const OPTION: u32 = 1 << 11;
const CONTROL: u32 = 1 << 12;

/// Modifiers and a key joined with "+", e.g. `cmd+space`, `ctrl+space`,
/// `cmd+shift+k`, `alt+f12`.
pub fn parse(spec: &str) -> Result<Hotkey, String> {
    let mut parts: Vec<String> = spec.split('+').map(|p| p.trim().to_ascii_lowercase()).collect();
    let name = parts.pop().filter(|k| !k.is_empty()).ok_or_else(|| format!("no key in hotkey '{spec}'"))?;
    let mut modifiers = 0;
    for m in &parts {
        modifiers |= match m.as_str() {
            "cmd" | "command" => CMD,
            "shift" => SHIFT,
            "alt" | "option" | "opt" => OPTION,
            "ctrl" | "control" => CONTROL,
            _ => return Err(format!("unknown modifier '{m}' in hotkey '{spec}' (cmd, ctrl, alt, shift)")),
        };
    }
    if modifiers == 0 {
        return Err(format!("hotkey '{spec}' needs a modifier, e.g. cmd+{name}"));
    }
    let key = key_code(&name).ok_or_else(|| format!("unknown key '{name}' in hotkey '{spec}'"))?;
    Ok(Hotkey { key, modifiers })
}

/// The ANSI keyboard's key codes for the names `parse` accepts.
fn key_code(name: &str) -> Option<u32> {
    const LETTERS: [u32; 26] = [
        0, 11, 8, 2, 14, 3, 5, 4, 34, 38, 40, 37, 46, 45, 31, 35, 12, 15, 1, 17, 32, 9, 13, 7, 16, 6,
    ];
    const DIGITS: [u32; 10] = [29, 18, 19, 20, 21, 23, 22, 26, 28, 25];
    const F_KEYS: [u32; 12] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111];
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_lowercase() {
            return Some(LETTERS[(c as u8 - b'a') as usize]);
        }
        if c.is_ascii_digit() {
            return Some(DIGITS[(c as u8 - b'0') as usize]);
        }
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()) {
        return F_KEYS.get(n.checked_sub(1)?).copied();
    }
    Some(match name {
        "space" => 49,
        "return" | "enter" => 36,
        "tab" => 48,
        "escape" | "esc" => 53,
        "grave" | "backtick" => 50,
        "comma" => 43,
        "period" => 47,
        "slash" => 44,
        "semicolon" => 41,
        _ => return None,
    })
}

type OSStatus = i32;
type EventTargetRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventHandlerProc = extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandlerProc,
        num_types: u32,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OSStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OSStatus;
}

const EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const EVENT_HOT_KEY_PRESSED: u32 = 5;

static ON_PRESS: OnceLock<fn()> = OnceLock::new();

extern "C" fn pressed(_: EventHandlerCallRef, _: EventRef, _: *mut c_void) -> OSStatus {
    if let Some(f) = ON_PRESS.get() {
        f();
    }
    0
}

/// Register `key` for the app's lifetime; `on_press` runs on the main thread,
/// from the app's event loop, at every press. Once per process. The error is
/// Carbon's status code.
pub fn register(key: Hotkey, on_press: fn()) -> Result<(), OSStatus> {
    ON_PRESS.set(on_press).map_err(|_| -1)?;
    let spec = EventTypeSpec { event_class: EVENT_CLASS_KEYBOARD, event_kind: EVENT_HOT_KEY_PRESSED };
    let id = EventHotKeyId { signature: u32::from_be_bytes(*b"skr1"), id: 1 };
    // SAFETY: plain Carbon calls with valid out-pointers; the handler and the
    // registration live as long as the process.
    unsafe {
        let target = GetApplicationEventTarget();
        let mut handler: EventHandlerRef = std::ptr::null_mut();
        let status = InstallEventHandler(target, pressed, 1, &spec, std::ptr::null_mut(), &mut handler);
        if status != 0 {
            return Err(status);
        }
        let mut hotkey: EventHotKeyRef = std::ptr::null_mut();
        let status = RegisterEventHotKey(key.key, key.modifiers, id, target, 0, &mut hotkey);
        if status != 0 {
            return Err(status);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs() {
        assert_eq!(parse("cmd+space"), Ok(Hotkey { key: 49, modifiers: CMD }));
        assert_eq!(parse("Ctrl + Space"), Ok(Hotkey { key: 49, modifiers: CONTROL }));
        assert_eq!(parse("cmd+shift+k"), Ok(Hotkey { key: 40, modifiers: CMD | SHIFT }));
        assert_eq!(parse("option+f12"), Ok(Hotkey { key: 111, modifiers: OPTION }));
        assert_eq!(parse("alt+1"), Ok(Hotkey { key: 18, modifiers: OPTION }));
        assert!(parse("space").is_err(), "a bare key would swallow it everywhere");
        assert!(parse("hyper+space").is_err());
        assert!(parse("cmd+").is_err());
        assert!(parse("cmd+f13").is_err());
    }

    #[test]
    fn every_letter_has_its_own_key() {
        let codes: std::collections::HashSet<u32> = ('a'..='z').map(|c| key_code(&c.to_string()).unwrap()).collect();
        assert_eq!(codes.len(), 26);
        assert_eq!(key_code("a"), Some(0));
        assert_eq!(key_code("z"), Some(6));
        assert_eq!(key_code("m"), Some(46));
    }
}
