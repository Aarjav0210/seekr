//! Key presses in the window as the finder's key events: what a terminal
//! sends for them, with ⌥ as Alt and ⌘ as Super (how the kitty launcher was
//! set up), so every binding in KEYBOARD_SHORTCUTS.md works unchanged.
use crate::tui::{KeyCode, KeyModifiers};

/// The modifier keys held during a press.
#[derive(Clone, Copy, Default, Debug)]
pub struct Held {
    pub shift: bool,
    pub control: bool,
    pub option: bool,
    pub command: bool,
}

/// The finder's event for a press of the key with virtual key code `code`
/// (Carbon's kVK_* numbers), which typed `chars`, or `plain` without the
/// modifiers other than shift. None for keys the finder has no use for.
pub fn translate(code: u16, chars: &str, plain: &str, held: Held) -> Option<(KeyCode, KeyModifiers)> {
    let mut mods = KeyModifiers::NONE;
    mods.set(KeyModifiers::SHIFT, held.shift);
    mods.set(KeyModifiers::CONTROL, held.control);
    mods.set(KeyModifiers::ALT, held.option);
    mods.set(KeyModifiers::SUPER, held.command);
    let key = match code {
        36 | 76 => KeyCode::Enter,
        48 if held.shift => KeyCode::BackTab,
        48 => KeyCode::Tab,
        51 => KeyCode::Backspace,
        117 => KeyCode::Delete,
        53 => KeyCode::Esc,
        123 => KeyCode::Left,
        124 => KeyCode::Right,
        125 => KeyCode::Down,
        126 => KeyCode::Up,
        115 => KeyCode::Home,
        119 => KeyCode::End,
        116 => KeyCode::PageUp,
        121 => KeyCode::PageDown,
        _ => {
            // With ⌃, ⌥ or ⌘ down the key's own character counts (⌥b would
            // otherwise type ∫), as from a terminal that sends Option as Alt.
            let chorded = held.control || held.option || held.command;
            let mut text = if chorded { plain } else { chars }.chars();
            let ch = text.next()?;
            // U+F700…U+F8FF: AppKit's codes for function keys (F1, Help, …).
            if text.next().is_some() || ch.is_control() || ('\u{f700}'..='\u{f8ff}').contains(&ch) {
                return None;
            }
            KeyCode::Char(if held.control { ch.to_ascii_lowercase() } else { ch })
        }
    };
    Some((key, mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Held = Held { shift: false, control: false, option: false, command: false };

    fn with(f: impl FnOnce(&mut Held)) -> Held {
        let mut h = NONE;
        f(&mut h);
        h
    }

    #[test]
    fn typing() {
        assert_eq!(translate(0, "a", "a", NONE), Some((KeyCode::Char('a'), KeyModifiers::NONE)));
        let shift = with(|h| h.shift = true);
        assert_eq!(translate(0, "A", "A", shift), Some((KeyCode::Char('A'), KeyModifiers::SHIFT)));
        assert_eq!(translate(49, " ", " ", NONE), Some((KeyCode::Char(' '), KeyModifiers::NONE)));
        assert_eq!(translate(122, "\u{f704}", "\u{f704}", NONE), None, "F1 types nothing");
    }

    #[test]
    fn chords_use_the_key_itself() {
        let ctrl = with(|h| h.control = true);
        // ⌃c types U+0003; the finder wants Char('c') with Control.
        assert_eq!(translate(8, "\u{3}", "c", ctrl), Some((KeyCode::Char('c'), KeyModifiers::CONTROL)));
        let alt = with(|h| h.option = true);
        assert_eq!(translate(11, "∫", "b", alt), Some((KeyCode::Char('b'), KeyModifiers::ALT)));
        let cmd = with(|h| h.command = true);
        assert_eq!(translate(123, "\u{f702}", "\u{f702}", cmd), Some((KeyCode::Left, KeyModifiers::SUPER)));
        assert_eq!(translate(51, "\u{7f}", "\u{7f}", cmd), Some((KeyCode::Backspace, KeyModifiers::SUPER)));
        assert_eq!(translate(51, "\u{7f}", "\u{7f}", alt), Some((KeyCode::Backspace, KeyModifiers::ALT)));
    }

    #[test]
    fn named_keys() {
        assert_eq!(translate(36, "\r", "\r", NONE).map(|k| k.0), Some(KeyCode::Enter));
        assert_eq!(translate(76, "\u{3}", "\u{3}", NONE).map(|k| k.0), Some(KeyCode::Enter));
        assert_eq!(translate(53, "\u{1b}", "\u{1b}", NONE).map(|k| k.0), Some(KeyCode::Esc));
        assert_eq!(translate(48, "\t", "\t", NONE).map(|k| k.0), Some(KeyCode::Tab));
        assert_eq!(translate(48, "\u{19}", "\t", with(|h| h.shift = true)).map(|k| k.0), Some(KeyCode::BackTab));
        assert_eq!(translate(125, "\u{f701}", "\u{f701}", NONE).map(|k| k.0), Some(KeyCode::Down));
    }

    #[test]
    fn nothing_for_dead_keys_and_compositions() {
        assert_eq!(translate(32, "", "u", NONE), None);
        assert_eq!(translate(0, "ab", "ab", NONE), None);
    }
}
